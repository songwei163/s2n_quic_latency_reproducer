use hbb_common::{
    bytes::Bytes,
    message_proto::{
        message, EncodedVideoFrame, EncodedVideoFrames, Message as FrameMessage, VideoFrame,
    },
    protobuf::Message,
    quic::{Connection, SendAPI, ReceiveAPI},
    tokio, ResultType,
};
use image::io::Reader as ImageReader;
use std::time::{self, Instant};
use std::{net::SocketAddr};
use futures::{
    sink::SinkExt,
    stream::{StreamExt},
};

use tracing::{info};
use tracing_subscriber;

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


#[tokio::main()]
async fn main() -> ResultType<()> {
    info!(
        "Start client demo, connecting the server {}:{}",
        SERVER_IP, SERVER_PORT
    );
    tracing_subscriber::fmt::init();
    let local_addr: SocketAddr = format!("{}:0", BIND_INTERFACE).parse().unwrap();
    let server_addr: SocketAddr = format!("{}:{}", SERVER_IP, SERVER_PORT).parse().unwrap();
    let mut conn = Connection::new_for_client_conn(server_addr, local_addr, 5000).await?;

    let img = ImageReader::open("image.png")?.decode()?;

    let fps = 25;
    let spf = time::Duration::from_secs_f32(1. / (fps as f32));
    let mut last_send_marked = time::Instant::now();

    // // let mut conn_sender = client_conn.get_conn_sender().await?;
    let mut conn_sender = conn.get_conn_sender().await?;
    tokio::spawn(async move {
        loop {
            if let Some(Ok(bytes)) = conn.next_timeout(5000).await {
                if let Ok(msg_in) = FrameMessage::parse_from_bytes(&bytes) {
                    match msg_in.union {
                        Some(message::Union::VideoFrame(vf)) => {
                            info!("E2E latency: {}", get_time() - vf.timestamp);
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
        conn_sender.send(&frames_msg).await.ok();
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
