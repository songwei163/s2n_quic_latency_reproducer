use hbb_common::{
    anyhow::{Context, anyhow},
    bytes::Bytes,
    message_proto::{
        message, EncodedVideoFrame, EncodedVideoFrames, Message as FrameMessage, VideoFrame,
    },
    protobuf::Message,
    quic::{Connection, ReceiveAPI, SendAPI},
    tokio, ResultType,
};
use image::io::Reader as ImageReader;
use std::time::{self, Instant};
use std::{
    net::SocketAddr,
    sync::Arc,
};

use hbb_common::{
    // message_proto::{message, Message as FrameMessage},
    // protobuf::Message,
    // quic::{server, Connection, ReceiveAPI, SendAPI},
    // ResultType, anyhow::anyhow,
    bytes_codec::BytesCodec
};
// use quinn::{Endpoint, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, info_span};
use tracing_subscriber;
use tokio_util::codec::{FramedRead, FramedWrite};
use futures::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};
use quinn::{ClientConfig, Endpoint, VarInt};
use std::{error::Error, net::ToSocketAddrs};
// use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::signal::unix::{signal, SignalKind};
// use url::Url;

const BIND_INTERFACE: &str = "0.0.0.0";
const SERVER_IP: &str = "127.0.0.1";
const SERVER_PORT: u32 = 12345;
const MARK_SEND_INTERVAL: u64 = 1;

#[inline]
pub fn get_time() -> i64 {
    std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis())
    .unwrap_or(0) as _
}

#[inline]
fn create_frame(frame: Vec<u8>) -> EncodedVideoFrame {
    EncodedVideoFrame {
        data: Bytes::from(frame),
        key: true,
        pts: 25,
        ..Default::default()
    }
}

#[inline]
fn create_msg(vp9s: Vec<EncodedVideoFrame>, last_send_marked: &mut Instant) -> FrameMessage {
    let mut msg_out = FrameMessage::new();
    let mut vf = VideoFrame::new();
    vf.set_vp9s(EncodedVideoFrames {
        frames: vp9s.into(),
        ..Default::default()
    });
    if last_send_marked.elapsed().as_secs() > MARK_SEND_INTERVAL {
        vf.timestamp = get_time();
        *last_send_marked = time::Instant::now();
    }
    msg_out.set_video_frame(vf);
    msg_out
}

/// Enables MTUD if supported by the operating system
#[cfg(not(any(windows, os = "linux")))]
pub fn enable_mtud_if_supported() -> quinn::TransportConfig {
    let transport_config = quinn::TransportConfig::default();
    transport_config
}

/// Enables MTUD if supported by the operating system
#[cfg(any(windows, os = "linux"))]
pub fn enable_mtud_if_supported() -> quinn::TransportConfig {
    let mut transport_config = quinn::TransportConfig::default();
    transport_config.mtu_discovery_config(Some(quinn::MtuDiscoveryConfig::default()));
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

        #[tokio::main()]
        async fn main() -> ResultType<()> {
        println!(
        "Start client demo, connecting the server {}:{}",
        SERVER_IP, SERVER_PORT
        );

        let local_addr: SocketAddr = format!("{}:0", BIND_INTERFACE).parse().unwrap();
        let server_addr: SocketAddr = format!("{}:{}", SERVER_IP, SERVER_PORT).parse().unwrap();

        // let mut client_conn = Connection::new_for_client_conn(server_addr, local_addr)
        //     .await
        //     .with_context(|| "Failed to neccect the server")?;

        let endpoint = make_client_endpoint(local_addr)?;
        let connection = endpoint
        .connect(server_addr, "localhost")?
        .await?;

        info!("11111");

        let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| anyhow!("failed to open stream: {}", e))?;
        info!("2222");
        let mut framed_recv = FramedRead::new(recv, BytesCodec::new());
        let mut framed_send = FramedWrite::new(send, BytesCodec::new());


        // loop {
        //     tokio::select! {
        //         bytes = framed_recv.next() => {
        //             if let Some(Ok(bytes)) = bytes {
        //                 if let Ok(msg_in) = FrameMessage::parse_from_bytes(&bytes) {
        //                     match msg_in.union {
        //                         Some(message::Union::VideoFrame(vf)) => {
        //                             println!("E2E latency: {}", get_time() - vf.timestamp);
        //                         }
        //                         _ => {}
        //                     }
        //                 }
        //             }
        //         }
        //     }
        // }


        let img = ImageReader::open("image.png")?.decode()?;

        let fps = 25;
        let spf = time::Duration::from_secs_f32(1. / (fps as f32));
        let mut last_send_marked = time::Instant::now();

        // let mut conn_sender = client_conn.get_conn_sender().await?;

        tokio::spawn(async move {
        loop {
        if let Some(Ok(bytes)) = framed_recv.next().await {
        if let Ok(msg_in) = FrameMessage::parse_from_bytes(&bytes) {
        match msg_in.union {
        Some(message::Union::VideoFrame(vf)) => {
        println!("E2E latency: {}", get_time() - vf.timestamp);
        }
        _ => {}
        }
        }
        }
        }
        });

        loop {
        let now = time::Instant::now();
        let frame_data = create_frame(img.clone().into_bytes());
        let mut frames = Vec::new();
        frames.push(frame_data);
        let frames_msg = create_msg(frames, &mut last_send_marked);
        let data = Bytes::from(frames_msg.write_to_bytes()?);
        framed_send.send(data).await.ok();
        info!("3333");
        let elapsed = now.elapsed();
        if elapsed < spf {
        tokio::time::sleep(spf - elapsed).await;
        // println!("Timestamp {:?}", time::Instant::now());
        } else {
        println!("Send slowly!!!");
        }
        }
        return Ok(());
        }
