use std::time::{self, Instant};
use anyhow::{anyhow};
use std::{net::SocketAddr, sync::Arc};
use crate::ResultType;
use quinn::{ClientConfig, Endpoint, VarInt};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use protobuf::Message;
use tokio_util::codec::{FramedRead, FramedWrite};
use quinn::{SendStream, RecvStream};
use std::{
    io::{BufReader, Error},
};
use crate::bytes_codec::BytesCodec;

use tokio::{
    self,
    sync::{mpsc, Mutex},
};

type Sender = mpsc::Sender<Bytes>;
pub type QuicFramedSend = FramedWrite<SendStream, BytesCodec>;
pub type QuicFramedRecv = FramedRead<RecvStream, BytesCodec>;
use futures::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};

const MAX_BUFFER_SIZE: usize = 128;

pub mod client {
    use super::*;
    /// Enables MTUD if supported by the operating system
    #[cfg(not(any(windows, os = "linux")))]
    pub fn enable_mtud_if_supported() -> quinn::TransportConfig {
        let transport_config = quinn::TransportConfig::default();
        transport_config
    }

    struct SkipServerVerification;

    impl SkipServerVerification {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    impl rustls::client::ServerCertVerifier for SkipServerVerification {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::Certificate,
            _intermediates: &[rustls::Certificate],
            _server_name: &rustls::ServerName,
            _scts: &mut dyn Iterator<Item = &[u8]>,
            _ocsp_response: &[u8],
            _now: std::time::SystemTime,
        ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::ServerCertVerified::assertion())
        }
    }

    fn configure_client() -> ResultType<ClientConfig> {
        let crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let mut client_config = ClientConfig::new(Arc::new(crypto));
        let mut transport_config = enable_mtud_if_supported();
        transport_config.max_idle_timeout(Some(VarInt::from_u32(60_000).into()));
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(1)));
        client_config.transport_config(Arc::new(transport_config));

        Ok(client_config)
    }

    /// Constructs a QUIC endpoint configured for use a client only.
    ///
    /// ## Args
    ///
    /// - server_certs: list of trusted certificates.
    #[allow(unused)]
    pub fn make_client_endpoint(bind_addr: SocketAddr) -> ResultType<Endpoint> {
        let client_cfg = configure_client()?;
        let mut endpoint = Endpoint::client(bind_addr)?;
        endpoint.set_default_client_config(client_cfg);
        Ok(endpoint)
    }
}

#[async_trait]
pub trait SendAPI {
    #[inline]
    async fn send(&mut self, msg: &impl Message) -> ResultType<()> {
        self.send_raw(msg.write_to_bytes()?).await
    }

    #[inline]
    async fn send_raw(&mut self, msg: Vec<u8>) -> ResultType<()> {
        let data = Bytes::from(msg);
        self.send_bytes(data).await
    }

    async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()>;
}

#[derive(Clone)]
pub struct ConnSender {
    sender: Sender,
}

#[async_trait]
impl SendAPI for ConnSender {
    #[inline]
    async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        self.sender
            .send(bytes)
            .await
            .map_err(|e| anyhow!("failed to send data: {}", e))?;
        Ok(())
    }
}

#[async_trait]
impl SendAPI for Connection {
    #[inline]
    async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        let mut lock = self.inner_sender.lock().await;
        if self.ms_timeout > 0 {
            super::timeout(self.ms_timeout, lock.send(bytes)).await??;
        } else {
            lock.send(bytes)
                .await
                .map(|e| anyhow!("failed to send data: {:?}", e))?;
        }
        Ok(())
    }
}

#[async_trait]
pub trait ReceiveAPI {
    async fn next(&mut self) -> Option<Result<BytesMut, Error>>;

    #[inline]
    async fn next_timeout(&mut self, ms: u64) -> Option<Result<BytesMut, Error>> {
        if let Ok(res) =
            tokio::time::timeout(std::time::Duration::from_millis(ms), self.next()).await
        {
            return res;
        }
        None
    }
}

#[async_trait]
impl ReceiveAPI for Connection {
    #[inline]
    async fn next(&mut self) -> Option<Result<BytesMut, Error>> {
        let res = self.inner_stream.next().await;
        res
    }
}

pub struct Connection {
    endpoint: Option<Endpoint>,  
    conn: Option<quinn::Connection>,  
    mpsc_sender: Option<Sender>,
    inner_sender: Arc<Mutex<QuicFramedSend>>,
    inner_stream: QuicFramedRecv,
    ms_timeout: u64,
}

impl Connection {
    pub async fn new_conn_wrapper(stream: (SendStream, RecvStream), ms_timeout: u64) -> ResultType<Self> {
        let sink_sender = FramedWrite::new(stream.0, BytesCodec::new());
        let inner_stream = FramedRead::new(stream.1, BytesCodec::new());
        let frame_sender = Arc::new(Mutex::new(sink_sender));
        let (conn_sender, mut conn_receiver) = mpsc::channel::<Bytes>(MAX_BUFFER_SIZE);
        let sender_ref = frame_sender.clone();
        tokio::spawn(async move {
            loop {
                match conn_receiver.recv().await {
                    Some(data) => {
                        let mut lock = sender_ref.lock().await;
                        if ms_timeout > 0 {
                            if let Err(e) =
                                super::timeout(ms_timeout, lock.send(data)).await
                            {
                                log::info!(
                                    "quic send failed from send channel timeout: {}",
                                    e
                                );
                                break;
                            }
                        } else {
                            if let Err(e) = lock.send(data).await {
                                log::info!("quic send failed from send channel: {}", e);
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
            log::error!("Conn sender exit loop!!!");
        });
        Ok(Connection { mpsc_sender: Some(conn_sender),inner_sender: frame_sender, inner_stream, endpoint: None, ms_timeout, conn: None })
    }

    pub async fn new_for_client_conn(
        server_addr: SocketAddr,
        local_addr: SocketAddr,
        ms_timeout: u64,
    ) -> ResultType<Self> {
        let endpoint = client::make_client_endpoint(local_addr)?;
        let connection = super::timeout(ms_timeout, endpoint.connect(server_addr, "localhost")?).await??;
        let stream = connection
        .open_bi()
        .await?;
        let mut conn = Connection::new_conn_wrapper(stream, ms_timeout).await?;
        conn.conn = Some(connection);
        conn.endpoint = Some(endpoint);
        Ok(conn)
    }

    pub async fn get_conn_sender(&self) -> ResultType<ConnSender> {
        if self.mpsc_sender.is_some() {
            let wrapper = ConnSender {
                sender: self.mpsc_sender.as_ref().unwrap().clone(),
            };
            return Ok(wrapper);
        }
        return Err(anyhow!("Not found the conn sender!!"));
    }

    #[inline]
    pub async fn shutdown(&mut self) -> ResultType<()> {
        self.inner_sender.lock().await.close().await?;
        
        if let Some(conn) = &self.conn {
            conn.close(0u32.into(), b"done");
        }
        // Give the peer a fair chance to receive the close packet
        if let Some(endpoint) = &self.endpoint {
            endpoint.wait_idle().await;
        }
        Ok(())
    }

}

pub mod server {
    use super::*;
    use quinn::{Endpoint, ServerConfig, VarInt};
    /// Returns default server configuration along with its certificate.
    fn configure_server() -> ResultType<(ServerConfig, Vec<u8>)> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = cert.serialize_der().unwrap();
        let priv_key = cert.serialize_private_key_der();
        let priv_key = rustls::PrivateKey(priv_key);
        let cert_chain = vec![rustls::Certificate(cert_der.clone())];

        let mut server_config = ServerConfig::with_single_cert(cert_chain, priv_key)?;
        let transport_config = Arc::get_mut(&mut server_config.transport).unwrap();
        transport_config.max_concurrent_uni_streams(0_u8.into());
        transport_config.max_idle_timeout(Some(VarInt::from_u32(60_000).into()));
        transport_config.keep_alive_interval(Some(std::time::Duration::from_secs(1)));
        #[cfg(any(windows, os = "linux"))]
        transport_config.mtu_discovery_config(Some(quinn::MtuDiscoveryConfig::default()));

        Ok((server_config, cert_der))
    }

    #[allow(unused)]
    pub fn make_server_endpoint(bind_addr: SocketAddr) -> ResultType<(Endpoint, Vec<u8>)> {
        let (server_config, server_cert) = configure_server()?;
        let endpoint = Endpoint::server(server_config, bind_addr)?;
        Ok((endpoint, server_cert))
    }

}