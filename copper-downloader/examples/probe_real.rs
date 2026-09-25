use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use copper_downloader::{DownloadEvent, DownloadManager, DownloadOptions, DownloadStatus};

#[tokio::main]
async fn main() {
    let url = std::env::args().nth(1).expect("url required");
    let dest = PathBuf::from(std::env::args().nth(2).expect("dest required"));

    let mgr = DownloadManager::new_with_proxy(2, tokio::runtime::Handle::current(), None);
    let last = Arc::new(Mutex::new((DownloadStatus::Queued, None::<String>, 0u64)));
    {
        let last = last.clone();
        mgr.add_listener(move |ev: DownloadEvent| {
            let s = match ev {
                DownloadEvent::Created(s)
                | DownloadEvent::Progress(s)
                | DownloadEvent::StatusChanged(s) => s,
            };
            let mut g = last.lock().unwrap();
            if g.0 != s.status || g.1 != s.error {
                println!(
                    "status={:?} err={:?} downloaded={} total={}",
                    s.status, s.error, s.downloaded_bytes, s.total_bytes
                );
            }
            *g = (s.status, s.error, s.downloaded_bytes);
        });
    }

    let id = mgr
        .enqueue(url.as_str(), dest.clone(), DownloadOptions::default())
        .expect("enqueue failed");
    println!("enqueued id={id} dest={}", dest.display());

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(40);
    loop {
        if std::time::Instant::now() > deadline {
            println!("TIMEOUT (40s)");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let s = last.lock().unwrap().0;
        if matches!(
            s,
            DownloadStatus::Done | DownloadStatus::Failed | DownloadStatus::Cancelled
        ) {
            break;
        }
    }
    if let Ok(snap) = mgr.snapshot(id) {
        println!(
            "FINAL status={:?} err={:?} downloaded={}",
            snap.status, snap.error, snap.downloaded_bytes
        );
    }
    mgr.cancel(id).ok();
}
