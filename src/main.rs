use std::{
    io::{BufRead, BufReader, Write},
    net::TcpListener,
};

enum StatusCode {
    Ok,
    NotFound,
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP/1.1 ")?;
        match self {
            Self::Ok => f.write_str("200 OK")?,
            Self::NotFound => f.write_str("404 Not Found")?,
        }
        f.write_str("\r\n\r\n")
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let buf_reader = BufReader::new(&stream);
                let received: Vec<String> = buf_reader
                    .lines()
                    .map_while(Result::ok)
                    .take_while(|x| !x.is_empty())
                    .collect();
                eprintln!("Received {received:#?}");

                let path = received
                    .first()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .expect("Path");

                let status_code = match path {
                    "/" => StatusCode::Ok,
                    _ => StatusCode::NotFound,
                };

                stream
                    .write_all(status_code.to_string().as_bytes())
                    .unwrap_or_else(|err| eprintln!("Failed to write: {err}"));
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}
