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
    let stream = TcpStream::connect("127.0.0.1:6606").await.unwrap();
    let (mut reader, mut _writer) = tokio::io::split(stream);

    loop {
        if let Some(dest) = read_package(&mut reader).await {
            let dest = String::from_utf8_lossy(&dest).to_string();
            let udp = Arc::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
            let _ = udp.connect("127.0.0.1:8080").await;
            if let Some(payload) = read_package(&mut reader).await {
                udp.send(&payload).await.unwrap();
            }
            let stream = TcpStream::connect("127.0.0.1:6607").await.unwrap();
            let (mut stream_reader, mut stream_writer) = tokio::io::split(stream);
            let identifier = build_package(Vec::new(), dest.parse().unwrap());
            stream_writer.write_all(&identifier[..]).await.unwrap();
            let udp1 = udp.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; u16::MAX as usize];
                loop {
                    let size = udp1.recv(&mut buf).await.unwrap();
                    let dest = dest.parse().unwrap();
                    //println!("client {dest}");
                    let package = build_package(buf[..size].to_owned(), dest);
                    //println!("read some payload from udp {}", package.len());
                    stream_writer.write_all(&package[..]).await.unwrap();
                    //println!("{r:?}");
                }
            });
            let udp2 = udp.clone();
            tokio::spawn(async move {
                loop {
                    if let Some(_) = read_package(&mut stream_reader).await {
                        if let Some(payload) = read_package(&mut stream_reader).await {
                            udp2.send(&payload).await.unwrap();
                        }
                    }
                }
            });
        }
    }
}
