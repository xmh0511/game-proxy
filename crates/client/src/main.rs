use std::{collections::HashMap, net::{SocketAddr, SocketAddrV4}, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf},
    net::{TcpListener, TcpStream, UdpSocket},
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
        if let Ok(read_size) = reader.read_exact(&mut buf).await {
            return Some(buf);
        }
        return None;
    }
    None
}

struct Response {
    payload: Vec<u8>,
    dest: String,
}

enum Message{
	Response(Response)
}

#[tokio::main]
async fn main() {
    let stream = TcpStream::connect("127.0.0.1:6606").await.unwrap();
	let (mut reader, mut writer) = tokio::io::split(stream);
	let mut remote_map = HashMap::new();
	let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Message>();
	tokio::spawn(async move {
		while let Some(msg) = receiver.recv().await{
			match msg{
				Message::Response(res)=>{
					let package = build_package(res.payload,res.dest.parse().unwrap());
					writer.write_all(&package).await.unwrap();
				}
			}
		}
	});
	loop{
		if let Some(dest) = read_package(& mut reader).await{
			let dest = String::from_utf8_lossy(&dest).to_string();
			if remote_map.get(&dest).is_none(){
				let udp = Arc::new(UdpSocket::bind("0.0.0.0:0").await.unwrap());
				let _ = udp.connect("127.0.0.1:8080").await;
				remote_map.insert(dest.clone(), udp.clone());
				if let Some(payload) = read_package(& mut reader).await{
					udp.send(&payload).await;
					let mut buf = [0u8;u16::MAX as usize];
					let sender2 = sender.clone();
					tokio::spawn(async move {
						loop{
							let size = udp.recv(& mut buf).await.unwrap();
							sender2.send(Message::Response(Response{
								dest:dest.clone(),
								payload:buf[..size].to_owned()
							})).unwrap();
						}
					});
				}
			}else{
				let udp = remote_map.get(&dest).unwrap().clone();
				if let Some(payload) = read_package(& mut reader).await{
					udp.send(&payload).await;
					// let mut buf = [0u8;u16::MAX as usize];
					// let sender2 = sender.clone();
					// tokio::spawn(async move {
					// 	let size = udp.recv(& mut buf).await.unwrap();
					// 	sender2.send(Message::Response(Response{
					// 		dest,
					// 		payload:buf[..size].to_owned()
					// 	})).unwrap();
					// });
				}
			}
		}
	}
}
