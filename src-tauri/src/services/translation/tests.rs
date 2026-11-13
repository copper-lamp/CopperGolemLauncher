use super::*;

#[test]
fn historical_google_default_migrates_but_custom_selection_stays() {
    let s=service();
    assert_eq!(s.config().unwrap().provider,"microsoft");
    let mut cfg=s.config().unwrap();cfg.provider="google".into();cfg.source_language="auto".into();
    s.save_config(cfg,HashMap::new()).unwrap();
    let migrated=TranslationService::new(s.db.clone(),s.settings.clone(),s.secret.clone()).unwrap();
    assert_eq!(migrated.config().unwrap().provider,"microsoft");
    let mut cfg=migrated.config().unwrap();cfg.provider="google".into();cfg.providers.insert("google".into(),ProviderConfig{endpoint:"https://example.org/translate".into(),..Default::default()});
    migrated.save_config(cfg,HashMap::new()).unwrap();
    let preserved=TranslationService::new(s.db.clone(),s.settings.clone(),s.secret.clone()).unwrap();
    assert_eq!(preserved.config().unwrap().provider,"google");
}

/// Explicit opt-in live evidence; never runs in the ordinary offline suite.
#[tokio::test]
#[ignore = "sends fixed public sample text to real translation services"]
async fn live_builtin_translation_and_cancel() {
    let s=service();
    let mut cfg=s.config().unwrap();cfg.cache_days=0;cfg.timeout_ms=10000;
    s.save_config(cfg,HashMap::new()).unwrap();
    let default_provider=s.config().unwrap().provider;
    let mut default_ok=false;
    for id in ["google","microsoft","indextranslate"] {
        let started=Instant::now();
        match s.test(id).await {
            Ok(value)=>{println!("LIVE provider={id} elapsed_ms={} translation={:?}",value.elapsed_ms,value.text);assert!(!value.text.trim().is_empty());if id==default_provider{default_ok=true;}},
            Err(error)=>println!("LIVE provider={id} elapsed_ms={} error={} retryable={}",started.elapsed().as_millis(),error.code,error.retryable),
        }
    }
    assert!(default_ok,"configured default must translate a real fixed sample");
    let mut req=request("live-market-sample","Download Minecraft resource packs and explore community content.");
    req.expected_generation=Some(s.config().unwrap().generation);
    let result=s.translate(req).await.expect("real default service content translation");
    println!("LIVE content provider={} translation={:?}",result.provider,result.texts);
    assert_ne!(result.texts[0],"Download Minecraft resource packs and explore community content.");
    let worker=s.clone();
    let pending=tokio::spawn(async move{worker.translate(request("live-cancel",&"This is a public translation cancellation sample. ".repeat(200))).await});
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    let started=Instant::now();
    assert!(s.cancel("live-cancel"),"request must remain registered until cancellation");
    let error=tokio::time::timeout(Duration::from_secs(2),pending).await.unwrap().unwrap().unwrap_err();
    assert_eq!(error.code,"translation.cancelled");
    println!("LIVE cancel error={} elapsed_ms={}",error.code,started.elapsed().as_millis());
    assert!(!s.cancel("live-cancel"));
}

use crate::platform::secret::SecretStore;
use crate::registry::events::EventBus;
use std::{io::{Read,Write},net::TcpListener,sync::atomic::{AtomicUsize,Ordering}};
#[derive(Default)]struct Secrets(Mutex<HashMap<String,String>>);
impl SecretStore for Secrets{
 fn set(&self,_:&str,a:&str,v:&str)->std::result::Result<(),String>{self.0.lock().insert(a.into(),v.into());Ok(())}
 fn get(&self,_:&str,a:&str)->std::result::Result<Option<String>,String>{Ok(self.0.lock().get(a).cloned())}
 fn delete(&self,_:&str,a:&str)->std::result::Result<(),String>{self.0.lock().remove(a);Ok(())}
}
fn service()->Arc<TranslationService>{
 let db=Arc::new(DatabaseService::open(std::path::Path::new(":memory:")).unwrap());
 db.migrate_scope("core",crate::services::database::CORE_MIGRATIONS).unwrap();
 let settings=Arc::new(SettingsService::new(db.clone(),Arc::new(EventBus::new()),HashMap::new()).unwrap());
 Arc::new(TranslationService::new(db,settings,Arc::new(Secrets::default())).unwrap())
}
fn server(status:u16,delay:u64)->(String,Arc<AtomicUsize>){server_body(status,delay,r#"{"translatedText":["translated"],"data":["translated"]}"#)}
fn server_body(status:u16,delay:u64,body:&'static str)->(String,Arc<AtomicUsize>){
 let listener=TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();
 let count=Arc::new(AtomicUsize::new(0));let observed=count.clone();
 std::thread::spawn(move||{
  // Listener nonblocking deadline avoids leaking a blocked server after cancellation tests.
  listener.set_nonblocking(true).unwrap();let deadline=Instant::now()+Duration::from_secs(8);
  while Instant::now()<deadline{
   match listener.accept(){Ok((mut stream,_))=>{let count=count.clone();std::thread::spawn(move||{
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut bytes=Vec::new();let mut buf=[0u8;8192];
    loop{match stream.read(&mut buf){Ok(0)|Err(_)=>return,Ok(n)=>bytes.extend_from_slice(&buf[..n])}
     if let Some(end)=bytes.windows(4).position(|v|v==b"\r\n\r\n"){
      let header=String::from_utf8_lossy(&bytes[..end]).to_lowercase();
      let len=header.lines().find_map(|line|line.strip_prefix("content-length:").and_then(|s|s.trim().parse::<usize>().ok())).unwrap_or(0);
      if bytes.len()>=end+4+len{break;}
     }
    }
    count.fetch_add(1,Ordering::SeqCst);std::thread::sleep(Duration::from_millis(delay));
    let body=if body=="bing-fixture"{if bytes.starts_with(b"GET "){r#"<html>IG:"test-ig", data-iid="translator.test"; var params_AbusePreventionHelper = [123,"test-token",999];</html>"#}else{r#"[{"translations":[{"text":"译文"}]}]"#}}else{body};
    let _=write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
   });},Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(5)),Err(_)=>break}
  }
 });
 (format!("http://{address}"),observed)
}
fn configured(s:&TranslationService,url:String){let mut cfg=s.config().unwrap();cfg.provider="libre".into();cfg.target_language="zh-CN".into();cfg.providers.insert("libre".into(),ProviderConfig{endpoint:url,..Default::default()});s.save_config(cfg,HashMap::new()).unwrap();}
fn request(id:&str,text:&str)->TranslationRequest{TranslationRequest{request_id:id.into(),texts:vec![text.into()],source_language:"en".into(),target_language:Some("zh-CN".into()),expected_generation:None}}
#[test]fn utf8_long_segments_preserve_every_byte(){for max in [4,450,1400,4000]{let text="汉字测试 and text.\n".repeat(3000);let chunks=split_text(&text,max);assert_eq!(chunks.concat(),text);assert!(chunks.iter().all(|s|s.len()<=max));}}
#[test]fn reference_registry_is_complete_and_native_is_honest(){let ids:Vec<_>=provider::registry().iter().map(|p|p.id).collect();assert_eq!(ids.len(),15);for id in ["indextranslate","deeplx","microsoft","azure","apple","auto"]{assert!(ids.contains(&id));}assert!(!provider::info("apple").unwrap().available);}
#[derive(Default)]struct UnavailableSecrets;
impl SecretStore for UnavailableSecrets{fn set(&self,_:&str,_:&str,_:&str)->std::result::Result<(),String>{Err("unavailable".into())}fn get(&self,_:&str,_:&str)->std::result::Result<Option<String>,String>{Err("unavailable".into())}fn delete(&self,_:&str,_:&str)->std::result::Result<(),String>{Err("unavailable".into())}}
#[test]fn public_endpoints_work_without_platform_secret_store(){let base=service();let s=TranslationService::new(base.db.clone(),base.settings.clone(),Arc::new(UnavailableSecrets)).unwrap();assert!(s.config().is_ok());assert_eq!(s.provider_key("google").unwrap(),"");assert_eq!(s.provider_key("deepl").unwrap_err().code,"translation.secret_store");}
#[test]fn cache_expires_and_generation_changes_identity(){let s=service();s.store("key","value",30,0).unwrap();assert_eq!(s.cached("key").unwrap().as_deref(),Some("value"));s.db.with_conn(|c|{c.execute("UPDATE core_translation_cache SET expires=?1",[now()-1])?;Ok(())}).unwrap();assert!(s.cached("key").unwrap().is_none());let p=ProviderConfig::default();assert_ne!(TranslationService::identity("google",&p,"en","zh","a",0),TranslationService::identity("google",&p,"en","zh","a",1));assert_eq!(s.store("stale","x",30,1).unwrap_err().code,"translation.configuration_changed");}
#[test]fn settings_failure_rolls_back_secret_changes(){let s=service();s.secret.set("copper-golem",&account("openai"),"old-value").unwrap();s.db.with_conn(|c|{c.execute_batch("CREATE TRIGGER deny_translation_settings BEFORE INSERT ON core_settings BEGIN SELECT RAISE(ABORT, 'test storage failure'); END;")?;Ok(())}).unwrap();let mut keys=HashMap::new();keys.insert("openai".into(),"new-value".into());assert!(s.save_config(s.config().unwrap(),keys).is_err());assert_eq!(s.secret.get("copper-golem",&account("openai")).unwrap().as_deref(),Some("old-value"));assert_eq!(s.config().unwrap().generation,0);}
#[test]fn configuration_conflicts_and_secrets_never_serialize(){let s=service();let stale=s.config().unwrap();let mut keys=HashMap::new();keys.insert("openai".into(),"secret-value".into());let saved=s.save_config(stale.clone(),keys).unwrap();assert!(saved.configured_keys.contains(&"openai".into()));assert!(!serde_json::to_string(&saved).unwrap().contains("secret-value"));assert_eq!(s.save_config(stale,HashMap::new()).unwrap_err().code,"translation.configuration_conflict");}
#[tokio::test]async fn success_cache_and_concurrent_dedup(){let s=service();let (url,count)=server(200,50);configured(&s,url);let (a,b)=tokio::join!(s.translate(request("a","hello")),s.translate(request("b","hello")));assert_eq!(a.unwrap().texts,vec!["translated"]);assert_eq!(b.unwrap().texts,vec!["translated"]);assert_eq!(count.load(Ordering::SeqCst),1);assert!(s.translate(request("c","hello")).await.unwrap().cached);assert_eq!(count.load(Ordering::SeqCst),1);}
#[tokio::test]async fn http_errors_are_not_cached(){let s=service();let (url,count)=server(400,0);configured(&s,url);assert_eq!(s.translate(request("a","hello")).await.unwrap_err().code,"translation.http_400");assert_eq!(s.translate(request("b","hello")).await.unwrap_err().code,"translation.http_400");assert_eq!(count.load(Ordering::SeqCst),2);}
#[tokio::test]async fn timeout_has_bounded_retry(){let s=service();let (url,count)=server(200,1600);configured(&s,url);let mut cfg=s.config().unwrap();cfg.timeout_ms=1000;s.save_config(cfg,HashMap::new()).unwrap();assert_eq!(s.translate(request("a","hello")).await.unwrap_err().code,"translation.timeout");assert_eq!(count.load(Ordering::SeqCst),2);}
#[tokio::test]async fn cancellation_drops_network_work(){let s=service();let (url,_)=server(200,2000);configured(&s,url);let worker=s.clone();let pending=tokio::spawn(async move{worker.translate(request("cancel-me","hello")).await});tokio::time::sleep(Duration::from_millis(50)).await;assert!(s.cancel("cancel-me"));assert_eq!(pending.await.unwrap().unwrap_err().code,"translation.cancelled");assert!(!s.cancel("cancel-me"));}
#[tokio::test]async fn long_input_is_complete_not_truncated(){let s=service();let (url,count)=server(200,0);configured(&s,url);let text="A".repeat(10000);let result=s.translate(request("long",&text)).await.unwrap();assert_eq!(result.texts[0],"translated".repeat(3));assert_eq!(count.load(Ordering::SeqCst),3);}
#[tokio::test]async fn protocols_send_and_decode_real_shapes(){
 for (id,body,model) in [
  ("google",r#"[[["译文","original",null,null,1]],null,"en"]"#,""),
  ("mymemory",r#"{"responseStatus":200,"responseData":{"translatedText":"译文"}}"#,""),
  ("deepl",r#"{"translations":[{"text":"译文"}]}"#,""),
  ("deeplx",r#"{"code":200,"data":["译文"]}"#,""),
  ("azure",r#"[{"translations":[{"text":"译文"}]}]"#,""),
  ("libre",r#"{"translatedText":["译文"]}"#,""),
  ("gemini",r#"{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"译文"}]}}]}"#,"test-model"),
  ("claude",r#"{"stop_reason":"end_turn","content":[{"type":"text","text":"译文"}]}"#,"test-model"),
  ("openai",r#"{"choices":[{"finish_reason":"stop","message":{"content":"译文"}}]}"#,"test-model"),
  ("deepseek",r#"{"choices":[{"finish_reason":"stop","message":{"content":"译文"}}]}"#,""),
  ("custom",r#"{"choices":[{"finish_reason":"stop","message":{"content":"译文"}}]}"#,""),
  ("indextranslate",r#"{"choices":[{"finish_reason":"stop","message":{"content":"译文"}}]}"#,""),
 ]{let (url,count)=server_body(200,0,body);let client=reqwest::Client::builder().no_proxy().build().unwrap();let cfg=ProviderConfig{endpoint:url,model:model.into(),..Default::default()};assert_eq!(provider::translate_piece(&client,id,&cfg,"test-key","original","en","zh-CN").await.unwrap(),"译文","{id}");assert_eq!(count.load(Ordering::SeqCst),1);}
}
#[tokio::test]async fn bing_bootstraps_then_posts(){let(url,count)=server_body(200,0,"bing-fixture");let client=reqwest::Client::builder().no_proxy().build().unwrap();assert_eq!(provider::translate_piece(&client,"microsoft",&ProviderConfig{endpoint:url,..Default::default()},"","original","auto","zh-CN").await.unwrap(),"译文");assert_eq!(count.load(Ordering::SeqCst),2);}
#[tokio::test]async fn missing_key_and_incomplete_output_are_errors(){let client=reqwest::Client::new();assert_eq!(provider::translate_piece(&client,"deepl",&ProviderConfig::default(),"","original","en","zh").await.unwrap_err().code,"translation.missing_key");let(url,_)=server_body(200,0,r#"{"choices":[{"finish_reason":"length","message":{"content":"partial"}}]}"#);assert_eq!(provider::translate_piece(&client,"custom",&ProviderConfig{endpoint:url,..Default::default()},"","original","en","zh").await.unwrap_err().code,"translation.incomplete_translation");}
#[tokio::test]async fn stale_expected_generation_never_contacts_provider(){let s=service();let(url,count)=server(200,0);configured(&s,url);let mut req=request("stale","hello");req.expected_generation=Some(0);assert_eq!(s.translate(req).await.unwrap_err().code,"translation.configuration_changed");assert_eq!(count.load(Ordering::SeqCst),0);assert!(s.requests.lock().is_empty());}
#[tokio::test]async fn saved_generation_cancels_previous_request(){let s=service();let(url,_)=server(200,1500);configured(&s,url);let worker=s.clone();let pending=tokio::spawn(async move{worker.translate(request("old-generation","hello")).await});tokio::time::sleep(Duration::from_millis(60)).await;let cfg=s.config().unwrap();s.save_config(cfg,HashMap::new()).unwrap();assert_eq!(pending.await.unwrap().unwrap_err().code,"translation.cancelled");}
#[tokio::test]async fn aborted_future_releases_request_registration(){let s=service();let(url,_)=server(200,1500);configured(&s,url);let worker=s.clone();let pending=tokio::spawn(async move{worker.translate(request("abort-me","hello")).await});tokio::time::sleep(Duration::from_millis(50)).await;pending.abort();let _=pending.await;assert!(!s.cancel("abort-me"));assert!(s.requests.lock().is_empty());}
#[tokio::test]async fn transient_failures_open_bounded_circuit(){let s=service();let(url,count)=server(503,0);configured(&s,url);assert_eq!(s.translate(request("a","hello")).await.unwrap_err().code,"translation.http_503");assert_eq!(s.translate(request("b","hello")).await.unwrap_err().code,"translation.circuit_open");assert_eq!(s.translate(request("c","hello")).await.unwrap_err().code,"translation.circuit_open");assert_eq!(count.load(Ordering::SeqCst),3);}
#[test]fn success_cache_survives_service_reconstruction(){let s=service();s.store("persisted","value",30,0).unwrap();let reopened=TranslationService::new(s.db.clone(),s.settings.clone(),s.secret.clone()).unwrap();assert_eq!(reopened.cached("persisted").unwrap().as_deref(),Some("value"));}
#[test]fn endpoint_rules_keep_credentials_off_plain_remote_http(){assert_eq!(provider::endpoint("custom",&ProviderConfig{endpoint:"http://remote.example/v1".into(),..Default::default()}).unwrap_err().code,"translation.https_required");assert!(provider::endpoint("custom",&ProviderConfig{endpoint:"http://127.0.0.1:8080/v1".into(),..Default::default()}).is_ok());assert!(provider::endpoint("custom",&ProviderConfig{endpoint:"https://user:password@example.com/v1".into(),..Default::default()}).is_err());}
#[tokio::test]async fn disabled_cache_still_deduplicates_inflight(){let s=service();let(url,count)=server(200,50);configured(&s,url);let mut cfg=s.config().unwrap();cfg.cache_days=0;s.save_config(cfg,HashMap::new()).unwrap();let(a,b)=tokio::join!(s.translate(request("a","hello")),s.translate(request("b","hello")));assert!(a.is_ok()&&b.is_ok());assert_eq!(count.load(Ordering::SeqCst),1);}
#[tokio::test]async fn explicit_sequential_fallback_only(){let s=service();let (bad,_)=server(400,0);let (good,count)=server(200,0);configured(&s,bad);let mut cfg=s.config().unwrap();cfg.fallback_providers=vec!["deeplx".into()];cfg.providers.insert("deeplx".into(),ProviderConfig{endpoint:good,..Default::default()});s.save_config(cfg,HashMap::new()).unwrap();let result=s.translate(request("fall","hello")).await.unwrap();assert_eq!(result.provider,"deeplx");assert_eq!(result.texts,vec!["translated"]);assert_eq!(count.load(Ordering::SeqCst),1);}
