use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
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
        f.write_str("\r\n")
    }
}

type HeaderMap<'a> = HashMap<&'a str, &'a str>;
struct Response<'a> {
    status: StatusCode,
    headers: Option<HeaderMap<'a>>,
    content: Option<&'a str>,
}

impl<'a> Response<'a> {
    fn build(status: StatusCode, headers: Option<HeaderMap<'a>>, content: Option<&'a str>) -> Self {
        Self {
            status,
            content,
            headers,
        }
    }
}

impl<'a> std::fmt::Display for Response<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}", self.status))?;

        if let Some(headers) = self.headers.as_ref() {
            for (key, value) in headers.iter() {
                f.write_fmt(format_args!("{}: {}\r\n", key, value))?
            }
        };

        f.write_str("\r\n")?;
        if let Some(content) = self.content {
            f.write_fmt(format_args!("{content}"))?;
        }
        Ok(())
        // f.write_str("\r\n")
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

                handle_request(&mut stream, &received).unwrap();
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}

fn handle_request(stream: &mut TcpStream, req: &[String]) -> anyhow::Result<()> {
    let path = req
        .first()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .expect("Path");

    match path {
        "/" => send_response(stream, Response::build(StatusCode::Ok, None, None)),
        _ if path.starts_with("/echo/") => {
            let content = path.strip_prefix("/echo/");
            let content_length = content.map_or(0, |x| x.len()).to_string();

            let mut headers = HashMap::new();
            headers.insert("Content-Type", "text/plain");
            headers.insert("Content-Length", &content_length);

            let response = Response::build(StatusCode::Ok, Some(headers), content);
            send_response(stream, response)
        }
        _ => send_response(stream, Response::build(StatusCode::NotFound, None, None)),
    }
}

fn send_response(stream: &mut TcpStream, response: Response) -> anyhow::Result<()> {
    stream
        .write_all(format!("{response}").as_bytes())
        .unwrap_or_else(|err| eprintln!("Failed to write: {err}"));
    Ok(())
}
