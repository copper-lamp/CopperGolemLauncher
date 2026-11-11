use super::*;
fn fixture() -> (Arc<SettingsService>, Arc<DatabaseService>, std::path::PathBuf) {
 let path=std::env::temp_dir().join(format!("settings-test-{}.db",uuid::Uuid::new_v4()));
 let db=Arc::new(DatabaseService::open(&path).unwrap());
 db.exec_batch("CREATE TABLE core_settings(key TEXT PRIMARY KEY,value TEXT NOT NULL)").unwrap();
 let service=Arc::new(SettingsService::new(db.clone(),Arc::new(EventBus::new()),HashMap::new()).unwrap());
 (service,db,path)
}
#[test]
fn failed_single_and_batch_never_publish_uncommitted_cache() {
 let (s,db,_path)=fixture();s.set("a",&1).unwrap();
 db.exec_batch("CREATE TRIGGER reject_settings BEFORE INSERT ON core_settings WHEN NEW.key='blocked' BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
 assert!(s.set("blocked",&99).is_err());assert_eq!(s.get::<i32>("blocked"),None);
 let entries=HashMap::from([("a".into(),Value::from(2)),("blocked".into(),Value::from(3))]);
 assert!(s.set_many(&entries).is_err());assert_eq!(s.get::<i32>("a"),Some(1));
 s.reload().unwrap();assert_eq!(s.get::<i32>("a"),Some(1));assert_eq!(s.get::<i32>("blocked"),None);
}
#[test]
fn concurrent_writers_leave_cache_equal_to_database() {
 let(s,_db,_path)=fixture();let mut threads=vec![];
 for n in 0..8 {let s=s.clone();threads.push(std::thread::spawn(move||for i in 0..40{s.set("value",&(n*100+i)).unwrap();}));}
 for t in threads{t.join().unwrap();}let cached=s.get::<i32>("value");s.reload().unwrap();assert_eq!(cached,s.get::<i32>("value"));
}
