use std::{
    collections::HashMap,
    ops::Deref,
    path::PathBuf,
    str::{from_utf8 as str_utf8, FromStr},
    sync::OnceLock,
};

use anyhow::Context;
use bytes::BytesMut;
use nom::{
    bytes::complete::{tag, take_till, take_while1},
    character::is_alphabetic,
    combinator,
    sequence::{preceded, tuple},
    IResult, Parser,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

struct Args {
    directory: Option<PathBuf>,
}

impl Args {
    fn parse() -> Self {
        let mut args = std::env::args();
        if args.len() < 2 {
            return Self { directory: None };
        }
        if args.nth(1).is_some_and(|x| x != "--directory") {
            panic!("Unsupported argument");
        }

        let directory = PathBuf::from(args.next().expect("Directory not missing"));
        Self {
            directory: Some(directory),
        }
    }
}

static ARGS: OnceLock<Args> = OnceLock::new();

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:4221").await.unwrap();

    ARGS.get_or_init(Args::parse);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(async move {
                    handle_connection(stream)
                        .await
                        .inspect_err(|err| eprintln!("{err}"))
                });
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }
}

async fn handle_connection(mut stream: TcpStream) -> anyhow::Result<()> {
    let mut bytes = BytesMut::with_capacity(1024);

    match stream.read_buf(&mut bytes).await {
        Ok(0) => return Err(anyhow::anyhow!("Read 0 bytes")),
        Ok(_) => (),
        Err(e) => return Err(anyhow::Error::from(e)),
    }

    let request = bytes.freeze();
    let request = Request::try_from(request.deref())?;
    handle_request(&mut stream, request).await
}

async fn handle_request(stream: &mut TcpStream, request: Request<'_>) -> anyhow::Result<()> {
    let mut response = Response::default();
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
        x if x.starts_with("/files/") => 'files: {
            let file_name = x.strip_prefix("/files/").expect("used starts_with");
            let directory = ARGS
                .get()
                .expect("Initialized")
                .directory
                .as_ref()
                .context("/files/ in path but no directory in arguments")?;
            let file_path = {
                let mut dir = directory.clone();
                dir.push(file_name);
                dir
            };

            match request.method {
                Methods::Get => {
                    let Ok(file_content) = tokio::fs::read(file_path).await else {
                        response.status = StatusCode::NotFound;
                        break 'files;
                    };
                    let content_length = file_content.len();
                    response
                        .header("Content-Type".into(), "application/octet-stream".into())
                        .header("Content-Length".into(), content_length.to_string());

                    response.content = file_content;
                }
                Methods::Post => {
                    tokio::fs::create_dir_all(&directory).await?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .create(true)
                        .open(file_path)
                        .await?;
                    file.write_all(&request.content).await?;
                    response.status = StatusCode::Created;
                }
            };
        }
        _ => response.status = StatusCode::NotFound,
    }
    send_response(stream, response).await
}

async fn send_response(stream: &mut TcpStream, response: Response) -> anyhow::Result<()> {
    stream
        .write_all(format!("{response}").as_bytes())
        .await
        .map_err(anyhow::Error::from)
}

#[derive(Debug)]
struct Request<'a> {
    #[allow(dead_code)]
    method: Methods,
    path: &'a str,
    // http_version:
    headers: HeaderMap,
    content: Vec<u8>,
}

impl<'a> Request<'a> {
    // https://github.com/rust-bakery/nom/blob/main/doc/making_a_new_parser_from_scratch.md
    fn get_start_line(input: &[u8]) -> IResult<&[u8], (Methods, &str)> {
        let method = take_while1(is_alphabetic);
        let space = nom::character::complete::space1::<&[u8], _>;
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
        })
        .parse(input)
    }

    fn get_header_lines(input: &[u8]) -> IResult<&[u8], HeaderMap> {
        let key = take_while1(|c| c != b':');
        let value = preceded(tag(": "), take_while1(|c| c != b'\r'));
        let line_ending = tag("\r\n");

        let comb = combinator::map_res(
            nom::sequence::tuple((key, value, line_ending)),
            |(key, value, _)| anyhow::Ok((str_utf8(key)?, str_utf8(value)?)),
        );
        nom::multi::fold_many0(comb, HashMap::new, |mut acc, (key, value)| {
            acc.insert(key.into(), value.into());
            acc
        })
        .parse(input)
    }

    fn get_content(input: &[u8]) -> IResult<&[u8], Vec<u8>> {
        // FIXME
        let mut content = preceded(tag("\r\n"), take_till(|_| false));
        let (input, content) = content.parse(input)?;
        Ok((input, content.to_vec()))
    }
}

impl<'a> TryFrom<&'a [u8]> for Request<'a> {
    type Error = anyhow::Error;

    fn try_from(request: &'a [u8]) -> Result<Self, Self::Error> {
        nom::combinator::map(
            nom::sequence::tuple((
                Self::get_start_line,
                Self::get_header_lines,
                Self::get_content,
            )),
            |((method, path), headers, content)| Request {
                method,
                path,
                headers,
                content,
            },
        )
        .parse(request)
        .map(|(_, request)| request)
        .map_err(|_| anyhow::anyhow!("Failed to parse request"))
    }
}

enum StatusCode {
    Ok,
    NotFound,
    Created,
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok => f.write_str("200 OK"),
            Self::NotFound => f.write_str("404 Not Found"),
            Self::Created => f.write_str("201 Created"),
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
    fn header(&mut self, key: String, value: String) -> &mut Self {
        self.headers.insert(key, value);
        self
    }
}

impl Default for Response {
    fn default() -> Self {
        Self {
            status: StatusCode::Ok,
            headers: HashMap::new(),
            content: Vec::new(),
        }
    }
}

impl std::fmt::Display for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("HTTP/1.1 {}\r\n", self.status))?;

        for (key, value) in self.headers.iter() {
            f.write_fmt(format_args!("{}: {}\r\n", key, value))?
        }

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
