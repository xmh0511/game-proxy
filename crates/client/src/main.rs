use std::{net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf},
    net::{TcpStream, UdpSocket},
};

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Public server's IP
    #[arg(short, long)]
    server: String,

    /// The control port between public server and this client
    #[arg(short, long)]
    control: u16,

    /// The data transport port between public server and this client
    #[arg(short, long)]
    transport: u16,

    /// which sevice this client proxy to
    #[arg(short = 'd', long)]
    target: String,

    /// how many numbers the client tries to connect server after disconnection
    #[arg(short, long, default_value_t = 5)]
    retry: u8,

    /// how long second we wait user to send the data each time
    #[arg(long, default_value_t = 60)]
    timeout: u64,
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

fn now_time() -> String {
    let now = chrono::Local::now();
    now.naive_local().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let Args {
        server: server_ip,
        control,
        transport,
        target,
        retry,
        timeout: time_out_seconds,
    } = args;
    // 控制服务端口
    let control_service_port: u16 = control;
    // 本地UDP服务
    let udp_service_target = target;
    // 创建对等连接端口
    let proxy_packet_port: u16 = transport;
    let mut count_retry = 0;

    'Reconn: loop {
        if count_retry > retry {
            panic!("fail to connect server within {retry} times");
        }
        //尝试连接服务器，如果失败直接报错
        let stream = match TcpStream::connect(format!("{server_ip}:{control_service_port}")).await {
            Ok(s) => {
                println!("连接{server_ip}:{control_service_port}服务器成功");
                count_retry = 0;
                s
            }
            Err(_) => {
                count_retry += 1;
                if count_retry <= retry {
                    println!("5秒后重新重新连接服务器");
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue 'Reconn;
            }
        };
        let (mut reader, mut _writer) = tokio::io::split(stream);

        loop {
            if let Ok(dest) = read_package(&mut reader).await {
                // 读取远程请求创建对应连接时发送的身份
                let dest = String::from_utf8_lossy(&dest).to_string();
                // 创建本地的对应实例
                let udp = Arc::new(if let Ok(udp) = UdpSocket::bind("0.0.0.0:0").await {
                    udp
                } else {
                    continue;
                });
                if let Err(_) = udp.connect(format!("{udp_service_target}")).await {
                    continue;
                }
                // 读取远程第一次的数据包，可能为空
                match read_package(&mut reader).await {
                    Ok(payload) => {
                        println!(
                            "{}首次从{dest}接受数据, 包大小{}字节",
                            now_time(),
                            payload.len()
                        );
                        if let Err(_) = udp.send(&payload).await {
                            continue;
                        }
                    }
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::NotConnected {
                            //panic!("an error occurs on the connection");
                            //尝试整个重新连接
                            continue 'Reconn;
                        }
                    }
                }
                let udp_copy = udp.clone();
                let use_server_ip = server_ip.clone();
                tokio::spawn(async move {
                    // 创建player到服务器时对应的代理到服务器的通道
                    let stream =
                        match TcpStream::connect(format!("{use_server_ip}:{proxy_packet_port}"))
                            .await
                        {
                            Ok(s) => s,
                            Err(_) => {
                                return;
                            }
                        };
                    let (mut stream_reader, mut stream_writer) = tokio::io::split(stream);
                    // 上报该连接对应的身份信息
                    let Ok(dest) = dest.parse::<SocketAddr>() else {
                        return;
                    };
                    let identifier = build_package(Vec::new(), dest);
                    if let Err(_) = stream_writer.write_all(&identifier[..]).await {
                        //发送身份信息失败
                        return;
                    }
                    let udp1 = udp_copy.clone();
                    // 接受本地UDP服务的响应并发送到远程服务器
                    tokio::spawn(async move {
                        let mut buf = [0u8; u16::MAX as usize];
                        loop {
                            let size = udp1.recv(&mut buf).await?;
                            //println!("client {dest}");
                            let package = build_package(buf[..size].to_owned(), dest);
                            //println!("read some payload from udp {}", package.len());
                            stream_writer.write_all(&package[..]).await?;
                            //println!("{r:?}");
                        }
                        #[allow(unreachable_code)]
                        Ok::<(), std::io::Error>(())
                    });
                    let udp2 = udp_copy.clone();
                    // 读取远程服务器代理到本地的UDP数据包
                    tokio::spawn(async move {
                        loop {
                            // 不应该超过1分钟没有数据请求
                            let timeout = tokio::time::timeout(
                                std::time::Duration::from_secs(time_out_seconds),
                                read_package(&mut stream_reader),
                            );
                            match timeout.await {
                                Ok(Ok(_)) => {
                                    //忽略传过来的身份信息，身份信息由闭包记录了
                                    let timeout = tokio::time::timeout(
                                        std::time::Duration::from_secs(time_out_seconds),
                                        read_package(&mut stream_reader),
                                    );
                                    match timeout.await {
                                        Ok(Ok(payload)) => {
                                            println!(
                                                "{} 从{dest}接收到数据包, 包大小{}字节",
                                                now_time(),
                                                payload.len()
                                            );
                                            if let Err(_) = udp2.send(&payload).await {
                                                break;
                                            }
                                        }
                                        e => {
                                            println!(
                                                "{} 与远程{dest}的数据交换通道异常，原因:{e:?}",
                                                now_time()
                                            );
                                            break;
                                        }
                                    }
                                }
                                e => {
                                    println!(
                                        "{} 与远程{dest}的数据交换通道异常，原因:{e:?}",
                                        now_time()
                                    );
                                    break;
                                }
                            };
                        }
                    });
                });
            } else {
                //panic!("an error occurs on the connection");
                continue 'Reconn;
            }
        }
    }
}
