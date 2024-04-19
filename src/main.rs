use std::{
    collections::HashMap,
    ops::Deref,
    str::{from_utf8 as str_utf8, FromStr},
};

use nom::{
    bytes::complete::{tag, take_while1},
    character::is_alphabetic,
    combinator,
    sequence::{preceded, tuple},
    IResult,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").await.unwrap();

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move { handle_connection(stream).await });
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream) -> anyhow::Result<()> {
    let mut bytes = bytes::BytesMut::with_capacity(1024);

    match stream.read_buf(&mut bytes).await {
        Ok(0) => return Err(anyhow::anyhow!("Read 0 bytes")),
        Ok(_) => (),
        Err(e) => return Err(anyhow::Error::from(e)),
    }

    let request = bytes.freeze();
    let request = Request::try_from(request.deref())?;
    handle_request(&mut stream, request).await?;
    Ok(())
}

async fn handle_request(stream: &mut TcpStream, request: Request<'_>) -> anyhow::Result<()> {
    let mut response = Response::new();
    match request.path {
        "/" => (),
        x if x.starts_with("/echo/") => {
            let content = x.strip_prefix("/echo/").expect("used starts_with");
            let content_length = content.len();

            response
                .header("Content-Type".into(), "text/plain".into())
                .header("Content-Length".into(), content_length.to_string());
            response.content = content.as_bytes().to_vec();
        }
        x if x.starts_with("/user-agent") => {
            let content = request
                .headers
                .get("User-Agent")
                .ok_or_else(|| anyhow::anyhow!("user-agent in path but not in headers"))?;
            let content_length = content.len();
            response
                .header("Content-Type".into(), "text/plain".into())
                .header("Content-Length".into(), content_length.to_string());
            response.content = content.clone().into_bytes();
        }
        _ => response.status = StatusCode::NotFound,
    }
    send_response(stream, response).await
}

async fn send_response(stream: &mut TcpStream, response: Response) -> anyhow::Result<()> {
    stream
        .write_all(format!("{response}").as_bytes())
        .await
        .unwrap_or_else(|err| eprintln!("Failed to write: {err}"));
    Ok(())
}

#[derive(Debug)]
struct Request<'a> {
    #[allow(dead_code)]
    method: Methods,
    path: &'a str,
    // http_version:
    headers: HeaderMap,
}

impl<'a> Request<'a> {
    // https://github.com/rust-bakery/nom/blob/main/doc/making_a_new_parser_from_scratch.md
    fn get_start_line(input: &[u8]) -> IResult<&[u8], (Methods, &str)> {
        let method = take_while1(is_alphabetic);
        let space = nom::character::complete::space0::<&[u8], _>;
        let path = take_while1(|c| c != b' ');

        let version = take_while1(|c: u8| (c.is_ascii_digit() || c == b'.'));
        let http_version = preceded(tag("HTTP/"), version);

        let line_ending = tag("\r\n");

        let tup = (method, space, path, space, http_version, line_ending);
        combinator::map_res(tuple(tup), |(method, _, path, _, _version, _)| {
            anyhow::Ok({
                let method = Methods::from_str(str_utf8(method)?)?;
                (method, str_utf8(path)?)
            })
        })(input)
    }

    fn get_header_line(input: &[u8]) -> IResult<&[u8], (&str, &str)> {
        let key = take_while1(|c| c != b':');
        let value = preceded(tag(": "), take_while1(|c| c != b'\r'));
        let line_ending = tag("\r\n");

        combinator::map_res(
            nom::sequence::tuple((key, value, line_ending)),
            |(key, value, _)| anyhow::Ok((str_utf8(key)?, str_utf8(value)?)),
        )(input)
    }
}

impl<'a> TryFrom<&'a [u8]> for Request<'a> {
    type Error = anyhow::Error;

    fn try_from(request: &'a [u8]) -> Result<Self, Self::Error> {
        let Ok((header_line, (method, path))) = Self::get_start_line(request) else {
            return Err(anyhow::anyhow!("Failed to parse start_line"));
        };

        let mut headers = HashMap::new();
        {
            let mut header_line = Self::get_header_line(header_line);
            while let Ok((header, (key, value))) = header_line {
                headers.insert(key.into(), value.into());
                header_line = Self::get_header_line(header);
            }
        }
        Ok(Self {
            method,
            path,
            headers,
        })
    }
}

enum StatusCode {
    Ok,
    NotFound,
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("200 OK"),
            Self::NotFound => f.write_str("404 Not Found"),
        }
    }
}

type HeaderMap = HashMap<String, String>;

struct Response {
    status: StatusCode,
    headers: HeaderMap,
    content: Vec<u8>,
}

impl Response {
    fn new() -> Self {
        Self {
            status: StatusCode::Ok,
            headers: HashMap::new(),
            content: Vec::new(),
        }
    }

    fn header(&mut self, key: String, value: String) -> &mut Self {
        self.headers.insert(key, value);
        self
    }
}

impl std::fmt::Display for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("HTTP/1.1 {}\r\n", self.status))?;

        if !self.headers.is_empty() {
            for (key, value) in self.headers.iter() {
                f.write_fmt(format_args!("{}: {}\r\n", key, value))?
            }
        };

        f.write_str("\r\n")?;
        if let Ok(content) = str_utf8(&self.content) {
            f.write_fmt(format_args!("{content}"))?;
        }
        Ok(())
    }
}

#[derive(Debug)]
enum Methods {
    Get,
    Post,
}

impl FromStr for Methods {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            s if s.eq_ignore_ascii_case("get") => Ok(Methods::Get),
            s if s.eq_ignore_ascii_case("post") => Ok(Methods::Post),
            _ => unimplemented!(),
        }
    }
}
