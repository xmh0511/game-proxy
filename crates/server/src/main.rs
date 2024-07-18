use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::{TcpListener, TcpStream, UdpSocket},
};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// The control port between public server and this client
    #[arg(short, long)]
    control: u16,

    /// The data transport port between public server and this client
    #[arg(short, long)]
    transport: u16,

    /// The public port for internet
	#[arg(short, long)]
    port: u16,
}


struct Client {
    payload: Vec<u8>,
    dest: SocketAddr,
    src: Arc<UdpSocket>,
}
#[derive(Debug)]
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
    RemoveUser(String),
    ConrolErr,
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

async fn read_package(reader: &mut ReadHalf<TcpStream>) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind};
    let mut buf_for_size = [0u8; 4];
    // 读取信息长度,i32大端表示
    if let Ok(header_size) = reader.read_exact(&mut buf_for_size).await {
        if header_size == 0 {
            return Err(Error::new(ErrorKind::NotConnected, ""));
        }
        let size = i32::from_be_bytes(buf_for_size);
        let mut buf = vec![0u8; size as usize];
        //读取信息主体
        if let Ok(_read_size) = reader.read_exact(&mut buf).await {
            return Ok(buf);
        }
        return Err(Error::new(ErrorKind::Other, "read body error"));
    }
    Err(Error::new(ErrorKind::Other, "read header error"))
}

macro_rules! dprintln {
	($($t:tt)*) => {
		if cfg!(debug_assertions){
			println!($($t)*);
		}
	};
}

#[tokio::main]
async fn main() {
	let args = Args::parse();
	let Args { control, transport, port } = args;
    let pub_service_port: u16 = port;
    let control_service_port: u16 = control;
	// 代理客户端和服务器之间代理数据的传输端口
    let proxy_packet_port: u16 = transport;
    // 服务器对公UDP服务
    let socket = UdpSocket::bind(format!("0.0.0.0:{pub_service_port}"))
        .await
        .unwrap();
    let socket = Arc::new(socket);
    // 限制用户数200
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Event>(200);
    let shared_sender = Arc::new(sender.clone());
    let weak_sender = Arc::downgrade(&shared_sender);
    tokio::spawn(async move {
        let mut control_stream = None;
        let mut user_map = HashMap::new();
        let mut peer_map = HashMap::<String, Peer>::new();
        let mut check_communication = HashMap::new();
        while let Some(event) = receiver.recv().await {
            match event {
                //代理的客户端和服务器进行控制层连接
                Event::Control(ctrl_writer) => {
                    control_stream = Some(ctrl_writer);
                }
                //公网用户数据包进来
                Event::UserSide(Client { payload, dest, src }) => {
                    let dest_str = dest.to_string();
                    //是否有记录，如果没有记录代表首次请求，记录到user_map中
                    if user_map.get(&dest_str).is_none() {
                        dprintln!("{}的首次连接", dest_str);
                        if let Some(stream) = &mut control_stream {
                            dprintln!("控制客户端创建对应连接");
                            //控制代理客户端创建对应的udp socket和数据包传输连接
                            let command = build_package(payload, dest);
                            //控制连接出现问题，那么情况所有状态，因为后续服务都不能正常提供
                            if let Err(_) = stream.write_all(&command[..]).await {
                                // 尝试关闭该控制连接
                                let _ = stream.shutdown().await;
                                control_stream = None;
                                user_map.clear();
                                peer_map.clear();
                                continue;
                            }
                            let sender_weak = weak_sender.clone();
                            let identifier = dest_str.clone();
                            check_communication.insert(
                                dest_str.clone(),
                                tokio::spawn(async move {
                                    //10秒内如果没有成功创建通信连接，则删除用户
                                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                                    println!("执行了remove操作");
                                    if let Some(sender) = sender_weak.upgrade() {
                                        _ = sender.send(Event::RemoveUser(identifier)).await;
                                    }
                                }),
                            );
                            user_map.insert(
                                dest_str,
                                Instance {
                                    dest,
                                    responder: src,
                                },
                            );
                            //println!("user_map = {user_map:?}");
                        }
                    } else {
                        // 已经记录到user_map中的，代表该用户的后续的数据包请求
                        // 通过成功创建的数据连接转发给代理客户端
                        if let Some(peer) = peer_map.get_mut(&dest_str) {
                            let command = build_package(payload, dest);
                            let writer = &mut peer.writer;
                            let timeout = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                writer.write_all(&command[..]),
                            );
                            match timeout.await {
                                Ok(Ok(_)) => {}
                                _ => {
                                    //连接出现错误，关闭连接
                                    let _ = writer.shutdown().await;
                                    //删除该连接
                                    peer_map.remove(&dest_str);
                                    // 从user_map中删除
                                    println!("从user_map中删除 {dest_str} at line {}", line!());
                                    user_map.remove(&dest_str);
                                }
                            }
                        }
                    }
                }
                Event::Response(res) => {
                    //dbg!(&res.dest,"收到该数据包");
                    dprintln!(
                        "收到了{}的数据，内容:{}",
                        res.dest,
                        String::from_utf8_lossy(&res.payload)
                    );
                    // 代理客户端发送过来的udp包
                    if let Some(ins) = user_map.get(&res.dest) {
                        // 转发给目标用户，完成转发
                        let r = ins.responder.send_to(&res.payload, &res.dest).await;
                        dprintln!("result is {r:?}");
                    } else {
                        dprintln!("没有为{}找到记录 user_map = {user_map:?}", res.dest);
                    }
                }
                Event::PeerCon(peer) => {
                    //代理客户端对用户首次UDP请求成功创建了映射和数据包连接
                    //取消检查计时
                    dprintln!("{} 完成了连接建立", peer.dest);
                    check_communication.get(&peer.dest).inspect(|h| h.abort());
                    peer_map.insert(peer.dest.clone(), peer);
                }
                Event::ConrolErr => {
                    //控制连接出现问题，清理所有信息
                    dprintln!("控制连接出现问题，清理所有信息");
                    if let Some(stream) = &mut control_stream {
                        let _ = stream.shutdown().await;
                    }
                    control_stream = None;
                    user_map.clear();
                    peer_map.clear();
                    check_communication
                        .iter()
                        .for_each(|(_key, val)| val.abort());
                    check_communication.clear();
                }
                Event::RemoveUser(dest) => {
                    dprintln!("从user_map中删除 {dest} at line {}", line!());
                    user_map.remove(&dest);
                    peer_map.remove(&dest);
                }
            }
        }
    });
    let sender2 = sender.clone();
    let sender3 = sender.clone();
    let tcp = TcpListener::bind(format!("0.0.0.0:{control_service_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _from)) = tcp.accept().await {
            dprintln!("new control connection!!!!");
            let (mut reader, writer) = tokio::io::split(stream);
            let _ = sender2.send(Event::Control(writer)).await;
            let sender3 = sender2.clone();
            tokio::spawn(async move {
                let mut buf = [0; 256];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(size) => {
                            // 代理服务器关闭了控制连接
                            if size == 0 {
                                let _ = sender3.send(Event::ConrolErr).await;
                                break;
                            }
                        }
                        Err(_) => {
                            let _ = sender3.send(Event::ConrolErr).await;
                            break;
                        }
                    }
                    // if let Some(dest) = read_package(&mut reader).await {
                    //     let dest = String::from_utf8_lossy(&dest).to_string();
                    //     if let Some(payload) = read_package(&mut reader).await {
                    //         let _ = sender3
                    //             .send(Event::Response(Response { payload, dest }))
                    //             .await;
                    //     }
                    // }
                }
            });
        }
    });

    // 代理客户端和服务器转发数据包的连接服务
    let tcp = TcpListener::bind(format!("0.0.0.0:{proxy_packet_port}"))
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _from)) = tcp.accept().await {
            let (mut reader, writer) = tokio::io::split(stream);
            //建立连接后，代理客户端向服务器发送身份信息
            match read_package(&mut reader).await {
                Ok(dest) => {
                    let _ = read_package(&mut reader).await; //empty payload
                    let dest = String::from_utf8_lossy(&dest).to_string();
                    if dest.is_empty() {}
                    //通道建立完成
                    let _ = sender3
                        .send(Event::PeerCon(Peer {
                            dest: dest.clone(),
                            writer,
                        }))
                        .await;
                    let sender4 = sender3.clone();
                    // 读取代理客户端向服务器发送UDP数据包的任务
                    let dest_record = dest;
                    tokio::spawn(async move {
                        loop {
                            let timeout = tokio::time::timeout(
                                std::time::Duration::from_secs(60),
                                read_package(&mut reader),
                            );
                            match timeout.await {
                                Ok(Ok(dest)) => {
                                    // 身份信息
                                    let dest = String::from_utf8_lossy(&dest).to_string();
                                    let timeout = tokio::time::timeout(
                                        std::time::Duration::from_secs(60),
                                        read_package(&mut reader),
                                    );
                                    //数据包
                                    match timeout.await {
                                        Ok(Ok(payload)) => {
                                            let _ = sender4
                                                .send(Event::Response(Response { payload, dest }))
                                                .await;
                                        }
                                        _ => {
                                            let _ = sender4
                                                .send(Event::RemoveUser(dest_record.clone()))
                                                .await;
                                            break;
                                        }
                                    }
                                }
                                _ => {
                                    let _ =
                                        sender4.send(Event::RemoveUser(dest_record.clone())).await;
                                    break;
                                }
                            }
                        }
                    });
                }
                Err(_) => {}
            }
        }
    });

    //对公用户进行UDP服务
    let mut buf = [0u8; u16::MAX as usize];
    while let Ok((size, from)) = socket.recv_from(&mut buf).await {
        dprintln!("udp packet from who {from}");
        let _ = sender
            .send(Event::UserSide(Client {
                payload: buf[..size].to_owned(),
                dest: from,
                src: socket.clone(),
            }))
            .await;
    }
}
