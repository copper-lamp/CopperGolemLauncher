//! Persistent metadata with a hard 72-hour lifetime and per-identity singleflight.
use std::{collections::HashMap, future::Future, path::{Path, PathBuf}, sync::{Arc, OnceLock, Weak}, time::{SystemTime, UNIX_EPOCH}};
use serde::{Serialize, de::DeserializeOwned};
use rusqlite::OptionalExtension;
use crate::error::KernelError;
pub const MAX_AGE: u64 = 72 * 3600;
pub const GAME_REFRESH: u64 = 24 * 3600;
pub const CONTENT_REFRESH: u64 = 12 * 3600;
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
fn flight(path: &Path, key: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<std::sync::Mutex<HashMap<(PathBuf,String),Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.retain(|_, v| v.strong_count() > 0);
    let id = (path.to_owned(), key.to_owned());
    if let Some(lock) = locks.get(&id).and_then(Weak::upgrade) { return lock; }
    let lock = Arc::new(tokio::sync::Mutex::new(())); locks.insert(id, Arc::downgrade(&lock)); lock
}
fn connection(root: &Path) -> Result<rusqlite::Connection, KernelError> {
    std::fs::create_dir_all(root)?;
    let conn = rusqlite::Connection::open(root.join("metadata.sqlite"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS metadata (identity TEXT PRIMARY KEY, validated INTEGER NOT NULL, value TEXT NOT NULL)")?;
    Ok(conn)
}
fn read<T: DeserializeOwned>(root: &Path, key: &str) -> Option<(u64,T)> {
    let conn = connection(root).ok()?;
    let row: Option<(u64,String)> = conn.query_row("SELECT validated,value FROM metadata WHERE identity=?1", [key], |r| Ok((r.get(0)?,r.get(1)?))).optional().ok()?;
    let (time, raw) = row?;
    // Future timestamps from a clock correction do not extend the hard TTL.
    if time > now() || now().saturating_sub(time) >= MAX_AGE { return None; }
    Some((time, serde_json::from_str(&raw).ok()?))
}
fn store<T: Serialize>(root: &Path, key: &str, value: &T) -> Result<(),KernelError> {
    let raw = serde_json::to_string(value)?;
    let mut conn = connection(root)?;
    let tx = conn.transaction()?;
    tx.execute("INSERT INTO metadata(identity,validated,value) VALUES (?1,?2,?3) ON CONFLICT(identity) DO UPDATE SET validated=excluded.validated,value=excluded.value", rusqlite::params![key,now(),raw])?;
    tx.execute("DELETE FROM metadata WHERE validated < ?1", [now().saturating_sub(MAX_AGE)])?;
    tx.commit()?; Ok(())
}
/// Identity must include provider, schema, platform, locale, and the entire query/page.
/// A failed fetch never updates the successful value or its timestamp.
pub async fn load<T,F,Fut>(root: &Path, key: &str, refresh_after: u64, fetch: F) -> Result<T,KernelError>
where T: Serialize + DeserializeOwned, F: FnOnce() -> Fut, Fut: Future<Output=Result<T,KernelError>> {
    let lock = flight(root,key); let _guard = lock.lock().await;
    let cached = read::<T>(root,key);
    if let Some((time,_)) = &cached { if now().saturating_sub(*time) < refresh_after { return Ok(cached.unwrap().1); } }
    match fetch().await {
        Ok(value) => { if let Err(e) = store(root,key,&value) { log::warn!("[metadata-cache] successful value could not be persisted: {e}"); } Ok(value) }
        Err(error) => match read::<T>(root,key) { Some((_,value)) => { log::warn!("[metadata-cache] refresh failed; using valid old metadata: {error}"); Ok(value) }, None => Err(error) }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf { std::env::temp_dir().join(format!("metadata-test-{}",uuid::Uuid::new_v4())) }
    #[tokio::test]
    async fn failure_preserves_value_and_timestamp_but_expiry_is_hard() {
        let root=root(); store(&root,"key",&vec![1]).unwrap();
        let conn=connection(&root).unwrap(); conn.execute("UPDATE metadata SET validated=?1",[now()-CONTENT_REFRESH-1]).unwrap();
        let old=read::<Vec<i32>>(&root,"key").unwrap().0;
        let v:Vec<i32>=load(&root,"key",CONTENT_REFRESH,||async{Err(KernelError::Config("offline".into()))}).await.unwrap(); assert_eq!(v,vec![1]); assert_eq!(read::<Vec<i32>>(&root,"key").unwrap().0,old);
        conn.execute("UPDATE metadata SET validated=?1",[now()-MAX_AGE]).unwrap();
        assert!(load::<Vec<i32>,_,_>(&root,"key",CONTENT_REFRESH,||async{Err(KernelError::Config("offline".into()))}).await.is_err()); drop(conn); std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn identical_requests_coalesce_but_pages_are_independent() {
        let root=root(); let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let request=|| { let calls=calls.clone(); let root=root.clone(); async move { load(&root,"provider/schema/platform/locale/page0",CONTENT_REFRESH,||async{ calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst); tokio::task::yield_now().await; Ok(vec![1]) }).await.unwrap() } };
        let (a,b)=tokio::join!(request(),request()); assert_eq!(a,b); assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst),1);
        let page:Vec<i32>=load(&root,"provider/schema/platform/locale/page1",CONTENT_REFRESH,||async{Ok(vec![2])}).await.unwrap(); assert_eq!(page,vec![2]); std::fs::remove_dir_all(root).unwrap();
    }
}
