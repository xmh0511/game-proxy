use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::{TcpListener, TcpStream, UdpSocket},
};

struct Client {
    payload: Vec<u8>,
    dest: SocketAddr,
    src: Arc<UdpSocket>,
}

struct Instance {
    #[allow(dead_code)]
    dest: SocketAddr,
    responder: Arc<UdpSocket>,
}

struct Response {
    payload: Vec<u8>,
    dest: String,
}

struct Peer {
    dest: String,
    writer: WriteHalf<TcpStream>,
}

enum Event {
    Control(WriteHalf<TcpStream>),
    UserSide(Client),
    Response(Response),
    PeerCon(Peer),
}
fn build_package(payload: Vec<u8>, dest: SocketAddr) -> Vec<u8> {
    let mut command = Vec::new();
    let dest_str = dest.to_string();
    let bytes = dest_str.as_bytes();
    let size = bytes.len() as i32;
    command.extend_from_slice(&size.to_be_bytes());
    command.extend_from_slice(bytes);
    let size = payload.len() as i32;
    command.extend_from_slice(&size.to_be_bytes());
    command.extend_from_slice(&payload);
    command
}

async fn read_package(reader: &mut ReadHalf<TcpStream>) -> Option<Vec<u8>> {
    let mut buf_for_size = [0u8; 4];
    if let Ok(4) = reader.read_exact(&mut buf_for_size).await {
        let size = i32::from_be_bytes(buf_for_size);
        let mut buf = vec![0u8; size as usize];
        if let Ok(_read_size) = reader.read_exact(&mut buf).await {
            return Some(buf);
        }
        return None;
    }
    None
}
#[tokio::main]
async fn main() {
    const SERVICE_PORT: u16 = 6600;
    const CONTROL_PORT: u16 = 6606;
    const CONN_PORT: u16 = 6607;
    let socket = UdpSocket::bind(format!("0.0.0.0:{SERVICE_PORT}"))
        .await
        .unwrap();
    let socket = Arc::new(socket);
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Event>(200);
    tokio::spawn(async move {
        let mut control_stream = None;
        let mut user_map = HashMap::new();
        let mut peer_map = HashMap::<String, Peer>::new();
        while let Some(event) = receiver.recv().await {
            match event {
                Event::Control(ctrl_writer) => {
                    control_stream = Some(ctrl_writer);
                }
                Event::UserSide(Client { payload, dest, src }) => {
                    let dest_str = dest.to_string();
                    if user_map.get(&dest_str).is_none() {
                        if let Some(stream) = &mut control_stream {
                            let command = build_package(payload, dest);
                            let _ = stream.write_all(&command[..]).await;
                            user_map.insert(
                                dest_str,
                                Instance {
                                    dest,
                                    responder: src,
                                },
                            );
                        }
                    } else {
                        if let Some(peer) = peer_map.get_mut(&dest_str) {
                            let command = build_package(payload, dest);
                            let writer = &mut peer.writer;
                            let _ = writer.write_all(&command[..]).await;
                        }
                    }
                }
                Event::Response(res) => {
                    //println!("response!!!!");
                    if let Some(ins) = user_map.get(&res.dest) {
                        let _ = ins.responder.send_to(&res.payload, &res.dest).await;
                    }
                }
                Event::PeerCon(peer) => {
                    peer_map.insert(peer.dest.clone(), peer);
                }
            }
        }
    });
    let sender2 = sender.clone();
    let sender3 = sender.clone();
    tokio::spawn(async move {
        let tcp = TcpListener::bind(format!("0.0.0.0:{CONTROL_PORT}"))
            .await
            .unwrap();
        while let Ok((stream, _from)) = tcp.accept().await {
            let (mut reader, writer) = tokio::io::split(stream);
            let _ = sender2.send(Event::Control(writer)).await;
            let sender3 = sender2.clone();
            tokio::spawn(async move {
                loop {
                    if let Some(dest) = read_package(&mut reader).await {
                        let dest = String::from_utf8_lossy(&dest).to_string();
                        if let Some(payload) = read_package(&mut reader).await {
                            let _ = sender3
                                .send(Event::Response(Response { payload, dest }))
                                .await;
                        }
                    }
                }
            });
        }
    });

    tokio::spawn(async move {
        let tcp = TcpListener::bind(format!("0.0.0.0:{CONN_PORT}"))
            .await
            .unwrap();
        while let Ok((stream, _from)) = tcp.accept().await {
            let (mut reader, writer) = tokio::io::split(stream);
            if let Some(dest) = read_package(&mut reader).await {
                let _ = read_package(&mut reader).await; //empty payload
                let dest = String::from_utf8_lossy(&dest).to_string();
                //println!("new conn {dest}");
                let _ = sender3
                    .send(Event::PeerCon(Peer {
                        dest: dest.clone(),
                        writer,
                    }))
                    .await;
                let sender4 = sender3.clone();
                tokio::spawn(async move {
                    loop {
                        if let Some(dest) = read_package(&mut reader).await {
                            let dest = String::from_utf8_lossy(&dest).to_string();
                            //println!("read from peer!!!! {dest}");
                            if let Some(payload) = read_package(&mut reader).await {
                                let _ = sender4
                                    .send(Event::Response(Response { payload, dest }))
                                    .await;
                            }
                        }
                    }
                });
            }
        }
    });

    let mut buf = [0u8; u16::MAX as usize];
    while let Ok((size, from)) = socket.recv_from(&mut buf).await {
        //println!("from who {from}");
        let _ = sender
            .send(Event::UserSide(Client {
                payload: buf[..size].to_owned(),
                dest: from,
                src: socket.clone(),
            }))
            .await;
    }
}
