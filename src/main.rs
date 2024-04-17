use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    str::FromStr,
};

use itertools::Itertools;
use tokio::task::JoinSet;

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

type HeaderMap = HashMap<String, String>;
struct Response<'a> {
    status: StatusCode,
    headers: Option<HeaderMap>,
    content: Option<&'a str>,
}

impl<'a> Response<'a> {
    fn build(status: StatusCode, headers: Option<HeaderMap>, content: Option<&'a str>) -> Self {
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
    }
}

#[derive(Debug)]
enum Methods {
    Get,
}

impl FromStr for Methods {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if s.eq_ignore_ascii_case("get") => Ok(Methods::Get),
            _ => unimplemented!(),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug)]
struct Request {
    method: Methods,
    path: String,
    http_version: String,
    headers: HeaderMap,
}

impl FromIterator<String> for Request {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        let mut iter = iter.into_iter();
        let start_line = iter.next().expect("Start line");
        let (method, path, http_version) = start_line
            .split_ascii_whitespace()
            .collect_tuple()
            .expect("Start line");

        let mut headers = HashMap::new();
        for header in iter {
            let (key, value) = header.split_once(": ").expect("Headers");
            headers.insert(key.into(), value.into());
        }

        Request {
            method: method.parse::<Methods>().expect("Valid method"),
            path: path.into(),
            http_version: http_version.into(),
            headers,
        }
    }
}

#[tokio::main]
async fn main() {
    let listener = TcpListener::bind("127.0.0.1:4221").unwrap();

    let mut set = JoinSet::new();
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                set.spawn(async move {handle_connection(stream)});
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }

    while let Some(res) = set.join_next().await {
        res.unwrap_or_else(|err| eprintln!("{err}"));
    }
}

fn handle_connection(mut stream: TcpStream) {
    let buf_reader = BufReader::new(&stream);
    let request: Request = buf_reader
        .lines()
        .map_while(Result::ok)
        .take_while(|x| !x.is_empty())
        .collect();
    eprintln!("Received {request:#?}");

    handle_request(&mut stream, request).unwrap();
}

fn handle_request(stream: &mut TcpStream, request: Request) -> anyhow::Result<()> {
    match request.path.as_str() {
        "/" => send_response(stream, Response::build(StatusCode::Ok, None, None)),
        x if x.starts_with("/echo/") => {
            let content = x.strip_prefix("/echo/");
            let content_length = content.map_or(0, |x| x.len()).to_string();

            let mut headers = HashMap::new();
            headers.insert("Content-Type".into(), "text/plain".into());
            headers.insert("Content-Length".into(), content_length);

            let response = Response::build(StatusCode::Ok, Some(headers), content);
            send_response(stream, response)
        }
        x if x.starts_with("/user-agent") => {
            let user_agent = request.headers.get("User-Agent").unwrap();
            let content_length = user_agent.len().to_string();

            let mut headers = HashMap::new();
            headers.insert("Content-Type".into(), "text/plain".into());
            headers.insert("Content-Length".into(), content_length);

            let response = Response::build(StatusCode::Ok, Some(headers), Some(user_agent));
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
