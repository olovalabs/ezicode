//! A real [`http_client::HttpClient`] backed by `zed-reqwest`.
//!
//! GPUI only ships a no-op `NullHttpClient`, so remote assets (Zed-style
//! blame avatars, loaded through the `img()` element) never fetch without
//! one. This mirrors Zed's `crates/reqwest_client`: a reqwest client that
//! drives every request on a dedicated tokio runtime (ezicode's main
//! executor is GPUI's smol one, not tokio's).

use std::mem;
use std::pin::Pin;
use std::sync::OnceLock;
use std::task::Poll;
use std::time::Duration;

use bytes::{BufMut, Bytes, BytesMut};
use futures::{AsyncRead, FutureExt as _, TryStreamExt as _};
use http_client::{RedirectPolicy, Url, http};

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub struct ReqwestClient {
    client: reqwest::Client,
    proxy: Option<Url>,
    user_agent: Option<http::HeaderValue>,
    handle: tokio::runtime::Handle,
}

impl ReqwestClient {
    fn builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .use_rustls_tls()
            .connect_timeout(Duration::from_secs(10))
            .tcp_keepalive(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(30))
            .http2_keep_alive_interval(Duration::from_secs(15))
            .http2_keep_alive_timeout(Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
    }

    /// A client with a Zed-flavoured user agent.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("Failed to initialize HTTP client")
            .into()
    }
}

impl Default for ReqwestClient {
    fn default() -> Self {
        Self::new()
    }
}

/// The tokio runtime every request is driven on. Created on demand
/// (GPUI apps have no tokio runtime of their own) and shared.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("Failed to initialize HTTP client")
    })
}

impl From<reqwest::Client> for ReqwestClient {
    fn from(client: reqwest::Client) -> Self {
        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| runtime().handle().clone());
        Self {
            client,
            handle,
            proxy: None,
            user_agent: None,
        }
    }
}
/// Re-implementation of tokio-util's `ReaderStream` outside of
/// tokio's aegis, so streamed request bodies work on GPUI's executor.
struct StreamReader {
    reader: Option<Pin<Box<dyn futures::AsyncRead + Send + Sync>>>,
    buf: BytesMut,
    capacity: usize,
}

impl StreamReader {
    fn new(reader: Pin<Box<dyn futures::AsyncRead + Send + Sync>>) -> Self {
        Self {
            reader: Some(reader),
            buf: BytesMut::new(),
            capacity: DEFAULT_CAPACITY,
        }
    }
}

const DEFAULT_CAPACITY: usize = 4096;

impl futures::Stream for StreamReader {
    type Item = std::io::Result<Bytes>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.as_mut();

        let mut reader = match this.reader.take() {
            Some(r) => r,
            None => return Poll::Ready(None),
        };

        if this.buf.capacity() == 0 {
            let capacity = this.capacity;
            this.buf.reserve(capacity);
        }

        match poll_read_buf(&mut reader, cx, &mut this.buf) {
            Poll::Pending => {
                self.reader = Some(reader);
                Poll::Pending
            }
            Poll::Ready(Err(err)) => {
                self.reader = None;
                Poll::Ready(Some(Err(err)))
            }
            Poll::Ready(Ok(0)) => {
                self.reader = None;
                Poll::Ready(None)
            }
            Poll::Ready(Ok(_)) => {
                let chunk = this.buf.split();
                self.reader = Some(reader);
                Poll::Ready(Some(Ok(chunk.freeze())))
            }
        }
    }
}

/// Specialized `poll_read_buf` (from tokio-util) for this use case.
fn poll_read_buf(
    io: &mut Pin<Box<dyn futures::AsyncRead + Send + Sync>>,
    cx: &mut std::task::Context<'_>,
    buf: &mut BytesMut,
) -> Poll<std::io::Result<usize>> {
    if !buf.has_remaining_mut() {
        return Poll::Ready(Ok(0));
    }

    let n = {
        let dst = buf.chunk_mut();
        let dst = unsafe { &mut *(dst as *mut _ as *mut [std::mem::MaybeUninit<u8>]) };
        let mut read_buf = tokio::io::ReadBuf::uninit(dst);
        let unfilled_portion = read_buf.initialize_unfilled();
        let io_pin = unsafe { Pin::new_unchecked(io) };
        std::task::ready!(io_pin.poll_read(cx, unfilled_portion)?)
    };

    unsafe {
        buf.advance_mut(n);
    }

    Poll::Ready(Ok(n))
}

/// Aborts the in-flight request when dropped (replaces Zed's
/// `gpui_util::defer`).
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl http_client::HttpClient for ReqwestClient {
    fn proxy(&self) -> Option<&Url> {
        self.proxy.as_ref()
    }

    fn user_agent(&self) -> Option<&http::HeaderValue> {
        self.user_agent.as_ref()
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn send(
        &self,
        req: http::Request<http_client::AsyncBody>,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<http_client::Response<http_client::AsyncBody>>,
    > {
        let (parts, body) = req.into_parts();

        let mut request_builder = self.client.request(parts.method, parts.uri.to_string());
        request_builder = request_builder.headers(parts.headers);
        if let Some(redirect_policy) = parts.extensions.get::<RedirectPolicy>() {
            request_builder = request_builder.redirect_policy(match redirect_policy {
                RedirectPolicy::NoFollow => reqwest::redirect::Policy::none(),
                RedirectPolicy::FollowLimit(limit) => {
                    reqwest::redirect::Policy::limited(*limit as usize)
                }
                RedirectPolicy::FollowAll => reqwest::redirect::Policy::limited(100),
            });
        }
        let request = request_builder.body(match body.0 {
            http_client::Inner::Empty => reqwest::Body::default(),
            http_client::Inner::Bytes(cursor) => cursor.into_inner().into(),
            http_client::Inner::AsyncReader(stream) => {
                reqwest::Body::wrap_stream(StreamReader::new(stream))
            }
        });

        let handle = self.handle.clone();
        async move {
            let join_handle = handle.spawn(async { request.send().await });
            let abort_handle = join_handle.abort_handle();
            let _abort_on_drop = AbortOnDrop(abort_handle);

            let mut response = join_handle.await??;

            let headers = mem::take(response.headers_mut());
            let mut builder =
                http::Response::builder().status(response.status().as_u16()).version(response.version());
            *builder.headers_mut().unwrap() = headers;

            let bytes = response
                .bytes_stream()
                .map_err(futures::io::Error::other)
                .into_async_read();
            let body = http_client::AsyncBody::from_reader(bytes);

            builder.body(body).map_err(|e| anyhow::anyhow!(e))
        }
        .boxed()
    }
}
