use std::{net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf},
    net::{TcpStream, UdpSocket},
};

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

#[tokio::main]
async fn main() {
    // 控制服务端口
    const CONTROL_SERVICE_PORT: u16 = 6606;
    // 本地UDP服务端口
    const UDP_SERVICE_PORT: u16 = 8080;
    // 创建对等连接端口
    const PROXY_PACKET_PORT: u16 = 6607;
    //尝试连接服务器，如果失败直接报错
    let stream = TcpStream::connect(format!("127.0.0.1:{CONTROL_SERVICE_PORT}"))
        .await
        .unwrap();
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
            if let Err(_) = udp.connect(format!("127.0.0.1:{UDP_SERVICE_PORT}")).await {
                continue;
            }
            // 读取远程第一次的数据包，可能为空
            match read_package(&mut reader).await {
                Ok(payload) => {
                    if let Err(_) = udp.send(&payload).await {
                        continue;
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotConnected {
                        panic!("an error occurs on the connection");
                    }
                }
            }
            // 创建player到服务器时对应的代理到服务器的通道
            let stream = TcpStream::connect(format!("127.0.0.1:{PROXY_PACKET_PORT}"))
                .await
                .unwrap();
            let (mut stream_reader, mut stream_writer) = tokio::io::split(stream);
            // 上报该连接对应的身份信息
            let Ok(dest) = dest.parse::<SocketAddr>() else {
                continue;
            };
            let identifier = build_package(Vec::new(), dest);
            if let Err(_) = stream_writer.write_all(&identifier[..]).await {
                //发送身份信息失败
                continue;
            }

            let udp1 = udp.clone();
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
            let udp2 = udp.clone();
            // 读取远程服务器代理到本地的UDP数据包
            tokio::spawn(async move {
                loop {
                    // 不应该超过1分钟没有数据请求
                    let timeout = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        read_package(&mut stream_reader),
                    );
                    if let Ok(Ok(_)) = timeout.await {
                        //忽略传过来的身份信息，身份信息由闭包记录了
                        let timeout = tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            read_package(&mut stream_reader),
                        );
                        match timeout.await {
                            Ok(Ok(payload)) => {
                                if let Err(_) = udp2.send(&payload).await {
                                    break;
                                }
                            }
                            _ => {
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
            });
        } else {
            panic!("an error occurs on the connection");
        }
    }
}
