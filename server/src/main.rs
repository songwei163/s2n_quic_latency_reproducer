use hbb_common::{
    message_proto::{message, Message as FrameMessage},
    protobuf::Message,
    quic::{server, ReceiveAPI, SendAPI, Connection},
    ResultType, anyhow::anyhow,
    bytes_codec::BytesCodec
};
use std::time;
use std::{
    net::SocketAddr,
};
use tracing::{error, info, info_span};
use tracing_subscriber;
use tokio_util::codec::{FramedRead, FramedWrite};
use futures::{
    sink::SinkExt,
    stream::{StreamExt},
};


async fn handle_connection(conn: quinn::Connecting) -> ResultType<()> {
    let connection = conn.await?;
    let _span = info_span!(
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
    stream: (quinn::SendStream, quinn::RecvStream),
) -> ResultType<()> {
    let mut conn = Connection::new_conn_wrapper(stream, 5000).await?;
    loop {
        tokio::select! {
            res = conn.next() => {
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
                                                conn.send_bytes(data).await.ok();
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
    let (endpoint, _) = server::make_server_endpoint(addr)?;
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