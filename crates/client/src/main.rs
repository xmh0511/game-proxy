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
    #[arg(long, default_value_t = 10)]
    timeout: u64,

    /// how long second we wait keepalive packet
    #[arg(long, default_value_t = 10)]
    interval: u64,

    /// The authorized key for control connection
    #[arg(short, long, default_value_t = String::from("88888888"))]
    key: String,
}

// 构造正常的命令包
// len:i32 identity len:i32 payload
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
    match reader.read_exact(&mut buf_for_size).await {
        Ok(header_size) => {
            if header_size == 0 {
                return Err(Error::new(ErrorKind::NotConnected, ""));
            }
            let size = i32::from_be_bytes(buf_for_size);
            let mut buf = vec![0u8; size as usize];
            //读取信息主体
            match reader.read_exact(&mut buf).await {
                Ok(_) => return Ok(buf),
                Err(e) => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("read body error {e:?} required read size {size}"),
                    ));
                }
            }
        }
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("read header error {e:?}"),
            ));
        }
    }
}

enum Comand {
    Ctrl(Vec<u8>),
    KeepAlive,
}

async fn read_command(reader: &mut ReadHalf<TcpStream>, timeout: u64) -> std::io::Result<Comand> {
    use std::io::{Error, ErrorKind};
    let mut buf_for_size = [0u8; 4];
    let task_time_out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout),
        reader.read_exact(&mut buf_for_size),
    );
    // 读取信息长度,i32大端表示
    match task_time_out.await {
        Ok(Ok(header_size)) => {
            if header_size == 0 {
                return Err(Error::new(ErrorKind::NotConnected, ""));
            }
            let size = i32::from_be_bytes(buf_for_size);
            //心跳包
            if size == 0 {
                return Ok(Comand::KeepAlive);
            }
            let mut buf = vec![0u8; size as usize];

            let body_task_time_out = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                reader.read_exact(&mut buf),
            );
            //读取信息主体
            match body_task_time_out.await {
                Ok(Ok(_)) => return Ok(Comand::Ctrl(buf)),
                e => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("read command body error {e:?} required read size {size}"),
                    ));
                }
            }
        }
        Ok(Err(e)) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("read command header error {e:?}"),
            ));
        }
        Err(e) => {
            return Err(Error::new(
                ErrorKind::Other,
                format!("read command header error {e:?}"),
            ));
        }
    }
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
        interval,
        key: connection_key,
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
            Ok(mut s) => {
                if let Err(_) = s.write_all(connection_key.as_bytes()).await {
                    continue 'Reconn;
                }
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

        // 向服务器写心跳包
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                let data = 0i32.to_be_bytes();
                match _writer.write_all(&data).await {
                    Ok(_) => {}
                    Err(_) => {
                        break;
                    }
                }
            }
        });

        loop {
            // 读取服务器发送过来的命令
            match read_command(&mut reader, interval).await {
                Ok(Comand::Ctrl(dest)) => {
                    // 读取远程请求创建对应连接时发送的身份
                    let dest = String::from_utf8_lossy(&dest).to_string();
                    println!("收到远程让客户端为{dest}创建数据通道的信号");
                    // 创建本地的对应实例
                    let udp = Arc::new(if let Ok(udp) = UdpSocket::bind("0.0.0.0:0").await {
                        udp
                    } else {
                        println!("为{dest}创建对等的UDP客户端失败");
                        continue;
                    });
                    if let Err(_) = udp.connect(format!("{udp_service_target}")).await {
                        println!("为{dest}创建对等的UDP客户端并进行connect时失败");
                        continue;
                    }
                    let first_read_body = tokio::time::timeout(
                        std::time::Duration::from_secs(time_out_seconds),
                        read_package(&mut reader),
                    );
                    // 读取远程第一次的数据包，可能为空
                    match first_read_body.await {
                        Ok(Ok(payload)) => {
                            println!(
                                "{}首次从{dest}接受数据, 包大小{}字节",
                                now_time(),
                                payload.len()
                            );
                            if let Err(_) = udp.send(&payload).await {
                                continue;
                            }
                        }
                        Ok(Err(e)) => {
                            println!(
                                "{}首次从{dest}接受数据出错，可能尝试整个重连 {:?}",
                                now_time(),
                                e
                            );
                            if e.kind() == std::io::ErrorKind::NotConnected {
                                //panic!("an error occurs on the connection");
                                //尝试整个重新连接
                                continue 'Reconn;
                            }
                        }
                        Err(e) => {
                            println!("{}首次从{dest}接受数据出错 {:?}", now_time(), e);
                        }
                    }
                    let udp_copy = udp.clone();
                    let use_server_ip = server_ip.clone();
                    tokio::spawn(async move {
                        // 创建player到服务器时对应的代理到服务器的通道
                        let stream = match TcpStream::connect(format!(
                            "{use_server_ip}:{proxy_packet_port}"
                        ))
                        .await
                        {
                            Ok(s) => s,
                            Err(e) => {
                                println!("为{dest}创建数据连接失败 {e:?}");
                                return;
                            }
                        };
                        let (mut stream_reader, mut stream_writer) = tokio::io::split(stream);
                        // 上报该连接对应的身份信息
                        let Ok(dest) = dest.parse::<SocketAddr>() else {
                            println!("获取的身份错误!!!!!!");
                            return;
                        };
                        let identifier = build_package(Vec::new(), dest);
                        if let Err(e) = stream_writer.write_all(&identifier[..]).await {
                            //发送身份信息失败
                            println!("发送身份信息{dest}失败 {e:?}");
                            return;
                        }
                        let udp1 = udp_copy.clone();
                        // 接受本地UDP服务的响应并发送到远程服务器
                        let writer_handler = tokio::spawn(async move {
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
                                                // println!(
                                                //     "{} 从{dest}接收到数据包, 包大小{}字节",
                                                //     now_time(),
                                                //     payload.len()
                                                // );
                                                if let Err(e) = udp2.send(&payload).await {
                                                    println!("发送{dest}包到本地udp失败 {e:?}");
                                                    break;
                                                }
                                            }
                                            e => {
                                                println!(
														"{} 与远程{dest}的数据交换通道异常，原因:{e:?} at {}",
														now_time(),
														line!()
													);
                                                break;
                                            }
                                        }
                                    }
                                    e => {
                                        println!(
                                            "{} 与远程{dest}的数据交换通道异常，原因:{e:?} at {}",
                                            now_time(),
                                            line!()
                                        );
                                        break;
                                    }
                                };
                            }
                            writer_handler.abort();
                        });
                    });
                }
                Ok(Comand::KeepAlive) => {
                    //心跳包
                    println!("{} 保活心跳包", now_time());
                }
                Err(e) => {
                    println!("control connection error {e:?}");
                    writer_task.abort();
                    continue 'Reconn;
                }
            }
        }
    }
}
