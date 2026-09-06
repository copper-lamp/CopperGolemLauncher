//! copper-downloader 集成测试：用本地 hyper 服务验证
//! 下载完成、断点续传（206 + Range）、暂停/恢复、取消、校验失败。
//! 服务端按小块 + 延时发送，模拟慢速网络，便于暂停 / 取消等竞态验证。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use copper_downloader::{
    DownloadManager, DownloadOptions, DownloadStatus,
};
use http_body_util::StreamBody;
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const CHUNK: usize = 8 * 1024;
const CHUNK_DELAY: Duration = Duration::from_millis(8);

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// 支持 Range、分块延时的本地静态文件服务。
async fn start_server(data: Arc<Vec<u8>>, ranges_seen: Arc<Mutex<Vec<String>>>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let data = data.clone();
            let ranges_seen = ranges_seen.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(move |req: Request<Incoming>| {
                    let data = data.clone();
                    let ranges_seen = ranges_seen.clone();
                    async move {
                        let range = req
                            .headers()
                            .get("range")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        if let Some(r) = &range {
                            ranges_seen.lock().push(r.clone());
                        }
                        let (status, start) = match &range {
                            Some(r) => {
                                let start: usize = r
                                    .trim_start_matches("bytes=")
                                    .trim_end_matches('-')
                                    .parse()
                                    .unwrap_or(0);
                                (StatusCode::PARTIAL_CONTENT, start)
                            }
                            None => (StatusCode::OK, 0),
                        };
                        let body = if start == 0 {
                            chunked_body(data.to_vec(), CHUNK, CHUNK_DELAY)
                        } else {
                            // 续传段直接全量发送（同样延时，保持节奏一致）。
                            chunked_body(data[start..].to_vec(), CHUNK, CHUNK_DELAY)
                        };
                        let len = data.len() - start;
                        let resp = Response::builder()
                            .status(status)
                            .header(
                                "Content-Range",
                                format!("bytes {}-{}/{}", start, data.len() - 1, data.len()),
                            )
                            .header("Content-Length", len)
                            .body(body)
                            .unwrap();
                        Ok::<_, hyper::Error>(resp)
                    }
                });
                let _ = AutoBuilder::new(TokioExecutor::new())
                    .serve_connection(io, svc)
                    .await;
            });
        }
    });
    addr
}

fn chunked_body(
    data: Vec<u8>,
    chunk: usize,
    delay: Duration,
) -> StreamBody<impl futures_util::Stream<Item = Result<Frame<Bytes>, Infallible>>> {
    let s = futures_util::stream::unfold((data, 0usize), move |(data, pos)| {
        let delay = delay;
        async move {
            if pos >= data.len() {
                return None;
            }
            tokio::time::sleep(delay).await;
            let end = (pos + chunk).min(data.len());
            let b = Bytes::from(data[pos..end].to_vec());
            Some((Ok::<_, Infallible>(Frame::data(b)), (data, end)))
        }
    });
    StreamBody::new(s)
}

fn manager() -> DownloadManager {
    DownloadManager::new(2, tokio::runtime::Handle::current())
}

async fn wait_status(mgr: &DownloadManager, id: u64, expect: DownloadStatus) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        match mgr.snapshot(id) {
            Ok(s) if s.status == expect => return,
            Ok(_) => {}
            Err(_) => return,
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "timeout waiting for {expect:?}, got: {:?}",
                mgr.snapshot(id)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn default_options() -> DownloadOptions {
    DownloadOptions {
        max_retries: 1,
        ..Default::default()
    }
}

#[tokio::test]
async fn download_completes_and_verifies() {
    let payload: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let addr = start_server(Arc::new(payload.clone()), ranges.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mgr = manager();
    let id = mgr
        .enqueue(
            format!("http://{addr}/file"),
            &dest,
            DownloadOptions {
                expected_sha256: Some(sha256_hex(&payload)),
                ..default_options()
            },
        )
        .unwrap();
    wait_status(&mgr, id, DownloadStatus::Done).await;
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    // 首次请求不应携带 Range。
    assert!(ranges.lock().is_empty());
}

#[tokio::test]
async fn pause_resume_resumes_from_range() {
    let payload: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let addr = start_server(Arc::new(payload.clone()), ranges.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mgr = manager();
    let id = mgr
        .enqueue(format!("http://{addr}/file"), &dest, default_options())
        .unwrap();

    // 等待进入下载中，随即暂停。
    wait_status(&mgr, id, DownloadStatus::Downloading).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    mgr.pause(id).unwrap();
    wait_status(&mgr, id, DownloadStatus::Paused).await;

    let paused_downloaded = mgr.snapshot(id).unwrap().downloaded_bytes;
    assert!(paused_downloaded > 0 && paused_downloaded < payload.len() as u64);

    mgr.resume(id).unwrap();
    wait_status(&mgr, id, DownloadStatus::Done).await;

    assert_eq!(std::fs::read(&dest).unwrap(), payload);
    // 恢复后的第二次请求应带 Range 续传，且从暂停位置开始（非 0）。
    let seen = ranges.lock().clone();
    assert_eq!(seen.len(), 1, "expected exactly one range request, seen: {seen:?}");
    assert!(seen[0].starts_with("bytes=") && !seen[0].starts_with("bytes=0-"));
}

#[tokio::test]
async fn cancel_removes_part_file() {
    let payload: Vec<u8> = vec![7u8; 400_000];
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let addr = start_server(Arc::new(payload.clone()), ranges.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mgr = manager();
    let id = mgr
        .enqueue(format!("http://{addr}/file"), &dest, default_options())
        .unwrap();

    wait_status(&mgr, id, DownloadStatus::Downloading).await;
    mgr.cancel(id).unwrap();
    wait_status(&mgr, id, DownloadStatus::Cancelled).await;
    // 取消后 .part 被删除。
    let part = std::path::PathBuf::from(format!("{}.part", dest.display()));
    assert!(!part.exists());
    assert!(!dest.exists());
}

#[tokio::test]
async fn checksum_mismatch_fails() {
    let payload: Vec<u8> = vec![3u8; 64_000];
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let addr = start_server(Arc::new(payload.clone()), ranges.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mgr = manager();
    let id = mgr
        .enqueue(
            format!("http://{addr}/file"),
            &dest,
            DownloadOptions {
                // 故意给错误校验和。
                expected_sha256: Some("0".repeat(64)),
                ..default_options()
            },
        )
        .unwrap();
    wait_status(&mgr, id, DownloadStatus::Failed).await;
    let snap = mgr.snapshot(id).unwrap();
    assert!(snap.error.unwrap_or_default().contains("校验失败"));
}

#[tokio::test]
async fn progress_events_are_emitted() {
    let payload: Vec<u8> = vec![9u8; 200_000];
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let addr = start_server(Arc::new(payload.clone()), ranges.clone()).await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mgr = manager();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    mgr.add_listener(move |ev| {
        let _ = tx.send(ev);
    });

    let id = mgr
        .enqueue(format!("http://{addr}/file"), &dest, default_options())
        .unwrap();
    wait_status(&mgr, id, DownloadStatus::Done).await;

    let mut saw_created = false;
    let mut saw_progress = false;
    let mut saw_done = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            copper_downloader::DownloadEvent::Created(_) => saw_created = true,
            copper_downloader::DownloadEvent::Progress(s) => {
                if s.downloaded_bytes > 0 {
                    saw_progress = true;
                }
            }
            copper_downloader::DownloadEvent::StatusChanged(s) => {
                if s.status == DownloadStatus::Done {
                    saw_done = true;
                }
            }
        }
    }
    assert!(saw_created && saw_progress && saw_done);
}
