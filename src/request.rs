use anyhow::{bail, ensure};
use bytes::Bytes;
use futures_util::TryStreamExt;
use http::{header::CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue, Method, Uri, Version};
use http_body::Frame;
use http_body_util::{combinators::BoxBody as _BB, BodyExt, StreamBody};
use nom::{
    bytes::complete::{tag, take_while1},
    character::{
        complete::{crlf, space1},
        is_alphabetic,
    },
    sequence::{preceded, terminated, tuple},
    IResult, Parser,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt};
use tokio_util::io::ReaderStream;

type BoxBody = _BB<Bytes, std::io::Error>;
pub struct RequestParser;

impl RequestParser {
    pub async fn parse<R>(mut reader: R) -> anyhow::Result<Request<BoxBody>>
    where
        R: AsyncBufReadExt + Unpin + Send + Sync + 'static,
    {
        let parts = {
            let mut buf = Vec::with_capacity(512);

            loop {
                if 0 == reader.read_until(b'\n', &mut buf).await? {
                    bail!("Incomplete request");
                }
                if buf.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            Parts::parse(buf)?
        };

        let body = {
            let content_length = parts
                .headers
                .get(CONTENT_LENGTH)
                .and_then(|x| x.to_str().ok())
                .and_then(|x| x.parse::<u64>().ok());

            content_length.filter(|x| *x != 0).map(|len| {
                let reader = reader.take(len);
                let stream = ReaderStream::new(reader);
                StreamBody::new(stream.map_ok(Frame::data)).boxed()
            })
        };
        Ok(Request { parts, body })
    }
}

#[derive(Debug)]
pub struct Request<T> {
    parts: Parts,
    pub(crate) body: Option<T>,
}

impl<T> Request<T> {
    #[inline]
    pub fn method(&self) -> &Method {
        &self.parts.method
    }

    #[inline]
    pub fn uri(&self) -> &Uri {
        &self.parts.uri
    }

    #[inline]
    pub fn version(&self) -> &Version {
        &self.parts.version
    }

    #[inline]
    pub fn headers(&self) -> &HeaderMap {
        &self.parts.headers
    }
}

#[derive(Debug, Clone)]
struct Parts {
    method: Method,
    uri: Uri,
    version: Version,
    headers: HeaderMap,
}

impl Parts {
    fn parse(bytes: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        nom::sequence::pair(Self::parse_start_line, Self::parse_header_lines)
            .parse(bytes.as_ref())
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to parse request: {:#?}",
                    e.map_input(Bytes::copy_from_slice)
                )
            })
            .and_then(|(rest, parsed)| {
                ensure!(rest.is_empty());
                Ok(parsed)
            })
            .map(|((method, uri, version), headers)| Self {
                method,
                uri,
                version,
                headers,
            })
    }

    // https://github.com/rust-bakery/nom/blob/main/doc/making_a_new_parser_from_scratch.md
    fn parse_start_line(input: &[u8]) -> IResult<&[u8], (Method, Uri, Version)> {
        let method = terminated(take_while1(is_alphabetic), space1);
        let uri = terminated(take_while1(|c| c != b' '), space1);

        let version = {
            let version = take_while1(|c: u8| c.is_ascii_digit() || c == b'.');
            terminated(preceded(tag(b"HTTP/"), version), crlf)
        };

        let tup = tuple((method, uri, version));
        nom::combinator::map_res(tup, |(method, uri, version)| {
            anyhow::Ok({
                let method = Method::from_bytes(method)?;
                let uri = Uri::try_from(uri)?;
                let version = match version {
                    b"1.1" => Version::HTTP_11,
                    _ => unimplemented!(),
                };
                (method, uri, version)
            })
        })
        .parse(input)
    }

    fn parse_header_lines(input: &[u8]) -> IResult<&[u8], HeaderMap> {
        let key_value = {
            let key = take_while1(|c| c != b':');
            let value = terminated(take_while1(|c| c != b'\r'), crlf);
            nom::sequence::separated_pair(key, tag(b": "), value)
        };

        let comb = nom::combinator::map_res(key_value, |(key, value)| {
            let key = HeaderName::try_from(key)?;
            let value = HeaderValue::try_from(value)?;
            anyhow::Ok((key, value))
        });

        let headers = nom::multi::fold_many0(comb, HeaderMap::new, |mut acc, (key, value)| {
            acc.insert(key, value);
            acc
        });

        terminated(headers, crlf).parse(input)
    }
}
