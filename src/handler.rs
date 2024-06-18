use std::io::Write;

use anyhow::{bail, Context};
use bstr::ByteSlice;
use bytes::Bytes;
use flate2::write::GzEncoder;
use futures_util::TryStreamExt;
use http::{
    header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, USER_AGENT},
    HeaderValue, Method, Response, StatusCode, Version,
};
use http_body::Frame;
use http_body_util::{combinators::BoxBody as _BB, BodyExt, StreamBody};
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::{Request, ARGUMENTS};

const TEXT_PLAIN: &str = "text/plain";
const OCTET_STREAM: &str = "application/octet-stream";
const GZIP: &str = "gzip";

type BoxBody = _BB<Bytes, std::io::Error>;

pub async fn handle_request<W>(request: Request<BoxBody>, responder: &mut W) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut path_parts = request
        .uri()
        .path()
        .split('/')
        .skip(1)
        .filter(|x| !x.is_empty());

    let accepts = request.headers().get_all(ACCEPT_ENCODING);

    let response = match path_parts.next().unwrap_or("") {
        "" => Response::new(BodyType::Empty),
        "echo" => {
            let arg = path_parts.next().context("Missing arg")?;

            let mut builder =
                Response::builder().header(CONTENT_TYPE, HeaderValue::from_static(TEXT_PLAIN));

            let content = if accepts
                .into_iter()
                .flat_map(|hv| hv.as_bytes().split_str(b", "))
                .any(|hv| hv.eq_ignore_ascii_case(b"gzip"))
            {
                builder = builder.header(CONTENT_ENCODING, HeaderValue::from_static(GZIP));
                encode_sync(arg)?
            } else {
                arg.as_bytes().to_owned()
            };
            builder
                .header(CONTENT_LENGTH, content.len())
                .body(BodyType::full(content))?
        }
        "user-agent" => request
            .headers()
            .get(USER_AGENT)
            .map(|content| {
                Response::builder()
                    .header(CONTENT_TYPE, HeaderValue::from_static(TEXT_PLAIN))
                    .header(CONTENT_LENGTH, content.len())
                    .body(BodyType::full(content.as_bytes().to_owned()))
            })
            .transpose()?
            .unwrap_or(not_found()),
        "files" => 'files: {
            let file_name = path_parts.next().context("Missing file name")?;
            let directory = ARGUMENTS
                .directory
                .as_ref()
                .context("/files/ in path but no directory in arguments")?;
            let file_path = directory.join(file_name);

            let builder = Response::builder();
            match *request.method() {
                Method::GET => {
                    let Ok(file) = tokio::fs::File::open(file_path).await else {
                        break 'files not_found();
                    };

                    builder
                        .header(CONTENT_TYPE, HeaderValue::from_static(OCTET_STREAM))
                        .header(CONTENT_LENGTH, file.metadata().await.map(|md| md.len())?)
                        .body(BodyType::chunked(file))?
                }
                Method::POST => {
                    let Some(body) = request.body else {
                        bail!("No body");
                    };
                    tokio::fs::create_dir_all(&directory).await?;
                    let mut file = tokio::fs::OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .create(true)
                        .open(file_path)
                        .await?;

                    write_body_to(&mut file, BodyType::Chunked(body)).await?;
                    builder.status(StatusCode::CREATED).body(BodyType::Empty)?
                }
                _ => unimplemented!(),
            }
        }
        _ => not_found(),
    };
    send_response(response, responder).await?;
    Ok(())
}

async fn send_response<W>(response: Response<BodyType>, writer: &mut W) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let (parts, body) = response.into_parts();

    writer.write_all(b"HTTP/").await?;
    match parts.version {
        Version::HTTP_09 => writer.write_all(b"0.9").await?,
        Version::HTTP_10 => writer.write_all(b"1.0").await?,
        Version::HTTP_11 => writer.write_all(b"1.1").await?,
        Version::HTTP_2 => writer.write_all(b"2.0").await?,
        Version::HTTP_3 => writer.write_all(b"3.0").await?,
        _ => unimplemented!(),
    }
    writer.write_u8(b' ').await?;

    writer.write_all(parts.status.as_str().as_bytes()).await?;
    if let Some(reason) = parts.status.canonical_reason() {
        writer.write_u8(b' ').await?;
        writer.write_all(reason.as_bytes()).await?;
    }
    writer.write_all(b"\r\n").await?;

    for (key, value) in parts.headers.iter() {
        writer.write_all(key.as_ref()).await?;
        writer.write_all(b": ").await?;
        writer.write_all(value.as_ref()).await?;
        writer.write_all(b"\r\n").await?;
    }
    writer.write_all(b"\r\n").await?;

    write_body_to(writer, body).await?;
    writer.flush().await?;
    Ok(())
}

async fn write_body_to<W>(writer: &mut W, body: BodyType) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    match body {
        BodyType::Full(inner) => writer.write_all(&inner).await?,
        BodyType::Chunked(mut inner) => {
            while let Some(chunk) = inner.frame().await {
                let chunk = chunk?;
                if let Ok(chunk) = chunk.into_data() {
                    writer.write_all(chunk.as_ref()).await?;
                }
            }
        }
        BodyType::Empty => return Ok(()),
    }

    writer.flush().await?;
    Ok(())
}

enum BodyType {
    Full(Bytes),
    Chunked(BoxBody),
    Empty,
}

impl BodyType {
    fn full<T: Into<Bytes>>(chunk: T) -> Self {
        Self::Full(chunk.into())
    }

    fn chunked<R>(r: R) -> Self
    where
        R: AsyncRead + Send + Sync + 'static,
    {
        let reader_stream = ReaderStream::new(r);
        let stream_body = StreamBody::new(reader_stream.map_ok(Frame::data));
        Self::Chunked(stream_body.boxed())
    }
}

fn not_found() -> Response<BodyType> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(BodyType::full("Not Found"))
        .unwrap()
}

fn encode_sync(bytes: impl AsRef<[u8]>) -> Result<Vec<u8>, std::io::Error> {
    let bytes = bytes.as_ref();
    let buf = Vec::new();
    let mut encoder = GzEncoder::new(buf, Default::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}
