use std::{
    io::{Read, Write},
    net::TcpListener,
};

fn main() {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let mut buf = [0_u8; 1024];
                let read = stream.read(&mut buf).unwrap();
                eprintln!("Read {read} bytes");
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\n\r\n")
                    .unwrap_or_else(|err| eprintln!("Failed to write: {err}"));
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}
