use hbb_common::{
    message_proto::{message, Message as FrameMessage},
    protobuf::Message,
    // quic::{server, Connection, ReceiveAPI, SendAPI},
    ResultType, anyhow::anyhow,
    bytes_codec::BytesCodec
};
use std::time;
use quinn::{Endpoint, ServerConfig, VarInt};
use std::{
    net::SocketAddr,
    sync::Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, info_span};
use tracing_subscriber;
use tokio_util::codec::{FramedRead, FramedWrite};
use futures::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};

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

async fn handle_connection(conn: quinn::Connecting) -> ResultType<()> {
    let connection = conn.await?;
    let span = info_span!(
        "connection",
        remote = %connection.remote_address(),
        protocol = %connection
            .handshake_data()
            .unwrap()
            .downcast::<quinn::crypto::rustls::HandshakeData>().unwrap()
            .protocol
            .map_or_else(|| "<none>".into(), |x| String::from_utf8_lossy(&x).into_owned())
    );

    // let incoming_streams = connection.incoming_bi_streams

    loop {
        match connection.accept_bi().await {
            Ok(stream) => {
                tokio::spawn(async move {
                    if let Err(e) = handle_request(stream).await {
                        error!("handle request failed: {:?}", e);
                    }
                });
            }
            Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                info!("connection closed");
                return Ok(());
            }
            Err(e) => {
                return Err(anyhow!("{:?}", e));
            }
        }
    }
}

async fn handle_request(
    (mut send, mut recv): (quinn::SendStream, quinn::RecvStream),
) -> ResultType<()> {
    let mut framed_recv = FramedRead::new(recv, BytesCodec::new());
    let mut framed_send = FramedWrite::new(send, BytesCodec::new()); 
    loop {
        tokio::select! {
            res = framed_recv.next() => {
                if let Some(res) = res {
                    match res {
                        Ok(bytes) => {
                            match FrameMessage::parse_from_bytes(&bytes) {
                                Ok(msg_in) => {
                                    match msg_in.union {
                                        Some(message::Union::VideoFrame(vf)) => {
                                            if vf.timestamp > 0 {
                                                let data = bytes.freeze();
                                                info!("forward the bytes received, timestamp: {:?}, size: {:?}", time::Instant::now(), data.len());
                                                framed_send.send(data).await.ok();
                                            }
                                        }
                                        _ => {
                                            error!("msg is null");
                                        }
                                    }
                                }
                                Err(e) => {
                                    return Err(anyhow!("parse failed {:?}", e));
                                }
                            }
                        }
                        Err(e) => {
                            return Err(anyhow!("{:?}", e)); 
                        }
                    }
                } else {
                    return Err(anyhow!("recv null"));
                }
            }
        }
    }
}

#[tokio::main()]
async fn main() -> ResultType<()> {
    tracing_subscriber::fmt::init();
    info!("->>>");
    let addr: SocketAddr = "0.0.0.0:12345".parse().unwrap();
    let (endpoint, _) = make_server_endpoint(addr)?;
    loop {
        tokio::select! {
            Some(incoming_conn) = endpoint.accept() => {
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(incoming_conn).await {
                        error!("handle_connection failed: {:?}", e);
                    }
                });
            }
        }
    }
    // Ok(())
}
// let port = 51111;
//     let bind_addr = format!("{}:{}", "0.0.0.0", port)
//         .parse()
//         .expect("parse signaling server address error");
//     let mut server_endpoint = server::new_server(bind_addr).expect("create server error");
//     println!("Start server demo, listening the port: {}", port);
//     loop {
//         tokio::select! {
//             Some(mut new_conn) = server_endpoint.accept() => {
//                 let client_addr = new_conn.remote_addr().unwrap();
//                 tokio::spawn(async move {
//                     println!("Found new connection, client addr: {:?}", &client_addr);
//                     while let Ok(Some(stream)) = new_conn.accept_bidirectional_stream().await {
//                         println!("Accept new bidirectional stream");
//                         let mut conn = Connection::new_conn_wrapper(stream).await.unwrap();
//                         tokio::spawn(async move {
//                             loop {
//                                 if let Some(result) = conn.next().await {
//                                     match result {
//                                         Err(err) => {
//                                             println!("read msg for peer {:?} with error: {:?}", &client_addr, err);
//                                             break;
//                                         }
//                                         Ok(bytes) => {
//                                             if let Ok(msg_in) = FrameMessage::parse_from_bytes(&bytes) {
//                                                 match msg_in.union {
//                                                     Some(message::Union::VideoFrame(vf)) => {
//                                                         if vf.timestamp > 0 {
//                                                             let data = bytes.freeze();
//                                                             println!("forward the bytes received, timestamp: {:?}, size: {:?}", time::Instant::now(), data.len());
//                                                             conn.send_bytes(data).await.ok();
//                                                         }
//                                                     }
//                                                     _ => {}
//                                                 }
//                                             }
//                                         }
//                                     }
//                                 }
//                             }
//                         });
//                     }
//                 });
//             }
//         }
//     }