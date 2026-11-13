//! On-demand translation service: secure credentials, sequential providers and bounded SQLite cache.
pub mod provider;
pub mod routing;
#[cfg(test)] mod tests;
use std::{collections::{BTreeMap,HashMap}, sync::Arc, time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use parking_lot::{Mutex,RwLock};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};
use sha2::{Digest,Sha256};
use tokio::sync::{watch,Semaphore};
use crate::{error::KernelError, platform::secret::SharedSecretStore,services::{database::{DatabaseService,Migration},settings::SettingsService}};

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TranslationError { pub code:String, pub retryable:bool }
impl TranslationError { pub fn new(code:&str,retryable:bool)->Self{Self{code:format!("translation.{code}"),retryable}} }
impl From<KernelError> for TranslationError {fn from(_:KernelError)->Self{Self::new("storage",false)}}
type Result<T> = std::result::Result<T,TranslationError>;
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
#[serde(default)]
pub struct ProviderConfig {pub endpoint:String,pub model:String,pub region:String,pub pro:bool}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default)]
pub struct TranslationConfig {
    pub provider:String,pub source_language:String,pub target_language:String,pub cache_days:u32,pub timeout_ms:u64,
    pub fallback_providers:Vec<String>,pub providers:BTreeMap<String,ProviderConfig>,
    pub generation:u64,
    #[serde(skip_deserializing)] pub configured_keys:Vec<String>,
}
impl Default for TranslationConfig {fn default()->Self{Self{provider:"microsoft".into(),source_language:"auto".into(),target_language:"auto".into(),cache_days:30,timeout_ms:25000,fallback_providers:Vec::new(),providers:BTreeMap::new(),generation:0,configured_keys:Vec::new()}}}
#[derive(Clone,Deserialize)]
pub struct TranslationRequest {pub request_id:String,pub texts:Vec<String>,pub source_language:String,pub target_language:Option<String>,pub expected_generation:Option<u64>,#[serde(default)] pub operation_id:Option<String>,#[serde(default)] pub operation_final:Option<bool>}
#[derive(Clone,Debug,Serialize)]
pub struct TranslationResponse {pub request_id:String,pub texts:Vec<String>,pub provider:String,pub generation:u64,pub cached:bool}
#[derive(Serialize)]
pub struct ConnectionTest {pub provider:String,pub text:String,pub elapsed_ms:u64}
struct Health {failures:u32,until:Instant,next_request:Instant}
pub struct TranslationService {
    db:Arc<DatabaseService>,settings:Arc<SettingsService>,secret:SharedSecretStore,
    config:RwLock<TranslationConfig>,save_lock:Mutex<()>,client:reqwest::Client,
    requests:Mutex<HashMap<String,watch::Sender<bool>>>,
    flights:Mutex<HashMap<String,std::sync::Weak<tokio::sync::Mutex<Option<String>>>>>,
    health:Mutex<HashMap<String,Health>>,network:Semaphore,provider_network:Mutex<HashMap<String,Arc<Semaphore>>>,
    routing:Mutex<routing::Routing>,request_operations:Mutex<HashMap<String,String>>,
}
struct CancelOnDrop {cancel:Option<watch::Sender<bool>>,armed:bool}
impl Drop for CancelOnDrop {fn drop(&mut self){if self.armed{if let Some(cancel)=&self.cancel{cancel.send_replace(true);}}}}
const MIGRATIONS:&[Migration]=&[Migration{version:1,name:"translation_cache",sql:"CREATE TABLE IF NOT EXISTS core_translation_cache (identity TEXT PRIMARY KEY, translated TEXT NOT NULL, expires INTEGER NOT NULL, accessed INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS translation_cache_expiry ON core_translation_cache(expires);"}];
fn now()->i64{SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64}
fn account(id:&str)->String{format!("translation.api_key:{id}")}
impl TranslationService {
    pub fn new(db:Arc<DatabaseService>,settings:Arc<SettingsService>,secret:SharedSecretStore)->std::result::Result<Self,KernelError>{
        db.migrate_scope("translation",MIGRATIONS)?;
        let mut config=settings.get_or("translation.config",TranslationConfig::default());
        config.configured_keys.clear();
        // Repair historical incomplete selections once, while retaining usable advanced settings and keys.
        let ready=|id:&str| {
            let Ok(info)=provider::info(id) else{return false;};
            if !info.available || id=="auto" || (!info.supports_auto_source && config.source_language=="auto"){return false;}
            let options=config.providers.get(id).cloned().unwrap_or_default();
            if provider::endpoint(id,&options).is_err(){return false;}
            !info.key_required || secret.get("copper-golem",&account(id)).ok().flatten().is_some_and(|key|!key.trim().is_empty())
        };
        let usable=if config.provider=="auto"{config.fallback_providers.iter().any(|id|ready(id))}else{ready(&config.provider)};
        // Historical Google defaults are unreachable in some supported networks. Move only
        // untouched single-provider selections; explicit endpoint/options/keys remain user-owned.
        let old_google_default=config.provider=="google" && config.source_language=="auto" && config.fallback_providers.is_empty()
            && config.providers.get("google").is_none_or(|p|(p.endpoint.trim().is_empty() || p.endpoint.trim()=="https://translate.googleapis.com/translate_a/single") && p.model.is_empty() && p.region.is_empty() && !p.pro)
            && config.providers.get("microsoft").is_none_or(|p|p.endpoint.is_empty() && p.model.is_empty() && p.region.is_empty() && !p.pro)
            && matches!(secret.get("copper-golem",&account("google")),Ok(None));
        if !usable || old_google_default {
            config.provider="microsoft".into();
            config.source_language="auto".into();
            config.fallback_providers.clear();
            if !usable {config.providers.remove("microsoft");}
            config.generation=config.generation.checked_add(1).ok_or_else(||KernelError::InvalidArgument("translation generation overflow".into()))?;
            settings.set("translation.config",&config)?;
        }
        let routing=Mutex::new(routing::Routing::load(&settings,config.generation));
        Ok(Self{routing,request_operations:Mutex::new(HashMap::new()),db,settings,secret,config:RwLock::new(config),save_lock:Mutex::new(()),client:super::http_client::client_builder(Duration::from_secs(60)).redirect(reqwest::redirect::Policy::none()).no_proxy().proxy(reqwest::Proxy::custom(|url|{if matches!(url.host_str(),Some("localhost"|"127.0.0.1"|"::1")){None}else{super::http_client::resolved_proxy()}})).build().map_err(|e|KernelError::InvalidArgument(format!("translation client: {e}")))?,requests:Mutex::new(HashMap::new()),flights:Mutex::new(HashMap::new()),health:Mutex::new(HashMap::new()),network:Semaphore::new(6),provider_network:Mutex::new(HashMap::new())})
    }
    pub fn config(&self)->Result<TranslationConfig>{
        let mut cfg=self.config.read().clone();
        for p in provider::registry().into_iter().filter(|p|p.id!="auto"&&p.id!="apple"){
            if self.secret.get("copper-golem",&account(p.id)).ok().flatten().is_some_and(|s|!s.is_empty()){cfg.configured_keys.push(p.id.into());}
        }
        Ok(cfg)
    }
    pub fn save_config(&self,mut cfg:TranslationConfig,keys:HashMap<String,String>)->Result<TranslationConfig>{
        let _guard=self.save_lock.lock();
        let current=self.config.read().clone();
        if cfg.generation!=current.generation{return Err(TranslationError::new("configuration_conflict",false));}
        provider::info(&cfg.provider)?;
        if cfg.cache_days>365 || !(1000..=60000).contains(&cfg.timeout_ms) || cfg.fallback_providers.len()>4 || cfg.target_language.len()>35 || cfg.source_language.len()>35 {return Err(TranslationError::new("invalid_configuration",false));}
        for id in &cfg.fallback_providers{if matches!(id.as_str(),"auto"|"apple"){return Err(TranslationError::new("invalid_fallback",false));}provider::info(id)?;}
        for (id,p) in &cfg.providers{provider::info(id)?;if !p.endpoint.is_empty(){provider::endpoint(id,p)?;}if p.model.len()>200||p.region.len()>100{return Err(TranslationError::new("invalid_configuration",false));}}
        // Validate and snapshot every secret before modifying any entry. Roll back on any persistence failure.
        let mut previous=Vec::new();
        for (id,value) in &keys {let p=provider::info(id)?;if matches!(p.id,"auto"|"apple")||value.len()>16384{return Err(TranslationError::new("invalid_key",false));}previous.push((id.clone(),self.secret.get("copper-golem",&account(id)).map_err(|_|TranslationError::new("secret_store",false))?));}
        let persist=||->Result<()>{
            for (id,value) in &keys {if value.is_empty(){self.secret.delete("copper-golem",&account(id))}else{self.secret.set("copper-golem",&account(id),value)}.map_err(|_|TranslationError::new("secret_store",false))?;}
            Ok(())
        };
        cfg.generation=current.generation.checked_add(1).ok_or_else(||TranslationError::new("generation_overflow",false))?;
        cfg.configured_keys.clear();
        let outcome=persist().and_then(|_|self.settings.set("translation.config",&cfg).map_err(Into::into));
        if let Err(error)=outcome {let mut rollback_failed=false;for (id,old) in previous{let result=if let Some(value)=old{self.secret.set("copper-golem",&account(&id),&value)}else{self.secret.delete("copper-golem",&account(&id))};rollback_failed|=result.is_err();}return Err(if rollback_failed{TranslationError::new("secret_rollback_failed",false)}else{error});}
        *self.config.write()=cfg;
        self.health.lock().clear();
        self.routing.lock().clear(self.config.read().generation);
        for cancel in self.requests.lock().values(){cancel.send_replace(true);}
        self.config()
    }
    fn provider_key(&self,id:&str)->Result<String>{
        match self.secret.get("copper-golem",&account(id)){
            Ok(value)=>Ok(value.unwrap_or_default()),
            Err(_) if !provider::info(id)?.key_required=>Ok(String::new()),
            Err(_)=>Err(TranslationError::new("secret_store",false)),
        }
    }
    pub fn routing_status(&self)->routing::Status{let generation=self.config.read().generation;self.routing.lock().status(generation)}
    pub fn cancel(&self,id:&str)->bool{
        let operation=self.request_operations.lock().get(id).cloned().unwrap_or_else(||id.into());
        self.routing.lock().cancel(&operation);
        let ids:Vec<_>=self.request_operations.lock().iter().filter(|(_,op)|**op==operation).map(|(id,_)|id.clone()).collect();
        let requests=self.requests.lock();let mut found=false;for id in ids{if let Some(tx)=requests.get(&id){tx.send_replace(true);found=true;}}found
    }
    pub fn clear_cache(&self)->Result<u64>{Ok(self.db.with_conn(|c|Ok(c.execute("DELETE FROM core_translation_cache",[])? as u64))?)}
    fn cached(&self,key:&str)->Result<Option<String>>{
        Ok(self.db.with_conn(|c|{
            let value=c.query_row("SELECT translated FROM core_translation_cache WHERE identity=?1 AND expires>?2",params![key,now()],|r|r.get(0)).optional()?;
            if value.is_some(){c.execute("UPDATE core_translation_cache SET accessed=?2 WHERE identity=?1",params![key,now()])?;}
            Ok(value)
        })?)
    }
    fn store(&self,key:&str,text:&str,days:u32,generation:u64)->Result<()>{
        // Keep the configuration read guard through the commit; save_config cannot publish a new generation meanwhile.
        let cfg=self.config.read();
        if cfg.generation!=generation{return Err(TranslationError::new("configuration_changed",false));}
        if days==0{return Ok(());}
        self.db.with_conn(|c|{let tx=c.transaction()?;
            tx.execute("DELETE FROM core_translation_cache WHERE expires<=?1",[now()])?;
            tx.execute("INSERT OR REPLACE INTO core_translation_cache VALUES (?1,?2,?3,?4)",params![key,text,now()+i64::from(days)*86400,now()])?;
            tx.execute("DELETE FROM core_translation_cache WHERE identity IN (SELECT identity FROM core_translation_cache ORDER BY accessed DESC LIMIT -1 OFFSET 10000)",[])?;
            tx.commit()?;Ok(())})?;Ok(())
    }
    fn identity(id:&str,p:&ProviderConfig,source:&str,target:&str,text:&str,generation:u64)->String{
        let encoded=serde_json::to_vec(&("translation-v1",id,p,source,target,text,generation)).expect("serializable cache identity");
        format!("{:x}",Sha256::digest(encoded))
    }
    async fn piece(&self,id:&str,p:&ProviderConfig,key:&str,text:&str,source:&str,target:&str,timeout:u64)->Result<String>{
        for attempt in 0..=1 {
            let delay={let mut all=self.health.lock();let state=all.entry(id.into()).or_insert_with(||Health{failures:0,until:Instant::now(),next_request:Instant::now()});
                if state.until>Instant::now(){return Err(TranslationError::new("circuit_open",true));}
                let scheduled=state.next_request.max(Instant::now());state.next_request=scheduled+Duration::from_millis(if id=="mymemory"{500}else{150});scheduled.saturating_duration_since(Instant::now())};
            tokio::time::sleep(delay).await;
            let provider_limit={self.provider_network.lock().entry(id.into()).or_insert_with(||Arc::new(Semaphore::new(2))).clone()};
            let _provider_permit=provider_limit.acquire().await.map_err(|_|TranslationError::new("shutdown",false))?;
            let result={let _permit=self.network.acquire().await.map_err(|_|TranslationError::new("shutdown",false))?;tokio::time::timeout(Duration::from_millis(timeout),provider::translate_piece(&self.client,id,p,key,text,source,target)).await.unwrap_or_else(|_|Err(TranslationError::new("timeout",true)))};
            drop(_provider_permit);
            {let mut all=self.health.lock();let state=all.entry(id.into()).or_insert_with(||Health{failures:0,until:Instant::now(),next_request:Instant::now()});match &result{Ok(_)=>state.failures=0,Err(e) if e.retryable=>{state.failures+=1;if state.failures>=3{state.until=Instant::now()+Duration::from_secs(30);}},_=>{}}}
            match result{Err(e) if e.retryable && attempt==0=>tokio::time::sleep(Duration::from_millis(300)).await,other=>return other,}
        }
        unreachable!()
    }
    async fn text(&self,id:&str,p:&ProviderConfig,key:&str,text:&str,source:&str,target:&str,cfg:&TranslationConfig)->Result<(String,bool)>{
        if text.trim().is_empty(){return Ok((text.into(),true));}
        let identity=Self::identity(id,p,source,target,text,cfg.generation);
        if cfg.cache_days>0 {if let Some(value)=self.cached(&identity)?{return Ok((value,true));}}
        let flight={let mut flights=self.flights.lock();flights.retain(|_,v|v.strong_count()>0);if let Some(v)=flights.get(&identity).and_then(std::sync::Weak::upgrade){v}else{let v=Arc::new(tokio::sync::Mutex::new(None));flights.insert(identity.clone(),Arc::downgrade(&v));v}};
        let mut flight_result=flight.lock().await;
        if let Some(value)=flight_result.as_ref(){return Ok((value.clone(),true));}
        if cfg.cache_days>0 {if let Some(value)=self.cached(&identity)?{return Ok((value,true));}}
        let max=provider::info(id)?.max_bytes;
        let mut translated=String::new();
        for part in split_text(text,max){
            if part.trim().is_empty(){translated.push_str(part);continue;}
            let leading=part.len()-part.trim_start().len();let trailing=part.trim_end().len();
            translated.push_str(&part[..leading]);
            translated.push_str(&self.piece(id,p,key,&part[leading..trailing],source,target,cfg.timeout_ms).await?);
            translated.push_str(&part[trailing..]);
        }
        if translated.trim().is_empty()||translated.trim()==text.trim()||translated.trim_start().to_ascii_lowercase().starts_with("<html")||translated.trim_start().to_ascii_lowercase().starts_with("<!doctype"){return Err(TranslationError::new("invalid_response",true));}
        self.store(&identity,&translated,cfg.cache_days,cfg.generation)?;
        *flight_result=Some(translated.clone());
        Ok((translated,false))
    }
    async fn run(self:&Arc<Self>,request:&TranslationRequest,cfg:&TranslationConfig,selected:Option<&str>)->Result<TranslationResponse>{
        let target=request.target_language.as_deref().filter(|s|!s.is_empty()).unwrap_or(&cfg.target_language);
        if target=="auto"||target.is_empty()||target.len()>35{return Err(TranslationError::new("target_language_required",false));}
        let mut ids=Vec::new();let selected=selected.unwrap_or(&cfg.provider);
        if selected!="auto"{ids.push(selected.to_owned());}
        if selected=="auto"||selected==cfg.provider{for id in &cfg.fallback_providers{if !ids.contains(id){ids.push(id.clone());}}}
        if ids.is_empty(){return Err(TranslationError::new("missing_fallback_configuration",false));}
        let mut last=TranslationError::new("unavailable",false);
        for id in ids {
            let p=cfg.providers.get(&id).cloned().unwrap_or_default();
            let key={let _save=self.save_lock.lock();if self.config.read().generation!=cfg.generation{return Err(TranslationError::new("configuration_changed",false));}self.provider_key(&id)};
            let key=match key{Ok(key)=>key,Err(error)=>{last=error;continue;}};
            let mut texts=vec![String::new();request.texts.len()];let mut cached=true;let mut error=None;
            let mut work=tokio::task::JoinSet::new();let mut next=0;
            loop {
                while work.len()<2&&next<request.texts.len(){let index=next;next+=1;let s=self.clone();let id=id.clone();let p=p.clone();let key=key.clone();let text=request.texts[index].clone();let source=request.source_language.clone();let target=target.to_owned();let cfg=cfg.clone();work.spawn(async move{(index,s.text(&id,&p,&key,&text,&source,&target,&cfg).await)});}
                let Some(result)=work.join_next().await else{break;};
                match result{Ok((index,Ok((value,hit))))=>{texts[index]=value;cached &=hit;},Ok((_,Err(e)))=>{error=Some(e);break;},Err(_)=>{error=Some(TranslationError::new("unavailable",true));break;}}
            }
            work.abort_all();while work.join_next().await.is_some(){}
            if let Some(e)=error{last=e;continue;}
            if self.config.read().generation!=cfg.generation{return Err(TranslationError::new("configuration_changed",false));}
            return Ok(TranslationResponse{request_id:request.request_id.clone(),texts,provider:id,generation:cfg.generation,cached});
        }
        Err(last)
    }
    fn candidates(&self,cfg:&TranslationConfig,source:&str)->Vec<String>{
        provider::registry().into_iter().filter(|p|p.available&&p.id!="auto"&&(source!="auto"||p.supports_auto_source)).filter(|p|{
            let options=cfg.providers.get(p.id).cloned().unwrap_or_default();
            (p.builtin_ready||p.id=="mymemory"||cfg.providers.contains_key(p.id)||cfg.provider==p.id||cfg.fallback_providers.iter().any(|id|id==p.id)||(p.key_required&&self.provider_key(p.id).is_ok_and(|key|!key.trim().is_empty())))
                && provider::endpoint(p.id,&options).is_ok() && (!p.key_required||self.provider_key(p.id).is_ok_and(|key|!key.trim().is_empty()))
        }).map(|p|p.id.into()).collect()
    }
    pub async fn translate(self:&Arc<Self>,request:TranslationRequest)->Result<TranslationResponse>{
        if request.request_id.is_empty()||request.request_id.len()>160||request.texts.len()>512||request.texts.iter().map(String::len).sum::<usize>()>2*1024*1024||request.source_language.len()>35{return Err(TranslationError::new("invalid_request",false));}
        let (cancel,mut cancelled)=watch::channel(false);
        {let mut requests=self.requests.lock();if requests.contains_key(&request.request_id){return Err(TranslationError::new("duplicate_request_id",false));}if requests.len()>=32{return Err(TranslationError::new("too_many_requests",true));}requests.insert(request.request_id.clone(),cancel);}
        let cfg=self.config.read().clone();
        if request.expected_generation.is_some_and(|generation|generation!=cfg.generation){self.requests.lock().remove(&request.request_id);return Err(TranslationError::new("configuration_changed",false));}
        let operation=request.operation_id.clone().unwrap_or_else(||request.request_id.clone());
        if operation.is_empty()||operation.len()>160{self.requests.lock().remove(&request.request_id);return Err(TranslationError::new("invalid_request",false));}
        let signature=format!("{}:{}",request.source_language,request.target_language.as_deref().unwrap_or(&cfg.target_language));
        let available=self.candidates(&cfg,&request.source_language);
        let routing=self.routing.lock().begin(&operation,signature,cfg.generation,available,request.operation_final.unwrap_or(true));
        let (ids,epoch)=match routing{Ok(value)=>value,Err(error)=>{self.routing.lock().cancel(&operation);self.requests.lock().remove(&request.request_id);return Err(error);}};
        self.request_operations.lock().insert(request.request_id.clone(),operation.clone());
        let mut guard=CancelOnDrop{cancel:self.requests.lock().get(&request.request_id).cloned(),armed:true};
        let service=self.clone();let (sender,receiver)=tokio::sync::oneshot::channel();
        tokio::spawn(async move{
            let mut tasks=tokio::task::JoinSet::new();let mut pending_ids=ids.clone();
            for id in ids {let s=service.clone();let req=request.clone();let mut single=cfg.clone();single.provider=id.clone();single.fallback_providers.clear();tasks.spawn(async move{let start=Instant::now();let result=s.run(&req,&single,Some(&id)).await; (id,start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,result)});}
            let mut sender=Some(sender);let mut last=TranslationError::new("unavailable",true);let mut succeeded=false;
            let deadline=tokio::time::sleep(Duration::from_secs(300));tokio::pin!(deadline);
            loop {tokio::select!{
                _=cancelled.changed()=>{service.routing.lock().cancel(&operation);last=TranslationError::new("cancelled",false);break;}
                _=&mut deadline=>{for id in &pending_ids{service.routing.lock().observe(&operation,id,epoch,300000,true,true,&service.settings);}last=TranslationError::new("deadline",true);break;}
                next=tasks.join_next()=>{let Some(next)=next else{break;};let Ok((id,elapsed,result))=next else{last=TranslationError::new("unavailable",true);continue;};
                    if *cancelled.borrow(){service.routing.lock().cancel(&operation);last=TranslationError::new("cancelled",false);break;}
                    if service.config.read().generation!=cfg.generation{service.routing.lock().cancel(&operation);last=TranslationError::new("configuration_changed",false);break;}
                    pending_ids.retain(|pending|pending!=&id);
                    match result {
                        Ok(value)=>{let valid=value.texts.iter().zip(&request.texts).all(|(out,input)|input.trim().is_empty()||(!out.trim().is_empty()&&!out.trim_start().starts_with("<!DOCTYPE")&&!out.trim_start().starts_with("<html")));
                            if !valid{service.routing.lock().observe(&operation,&id,epoch,elapsed,true,true,&service.settings);last=TranslationError::new("invalid_response",true);continue;}
                            let changed=value.texts.iter().zip(&request.texts).any(|(out,input)|out.trim()!=input.trim());
                            if !changed && request.texts.iter().any(|text|!text.trim().is_empty()){service.routing.lock().observe(&operation,&id,epoch,elapsed,!value.cached,!value.cached,&service.settings);last=TranslationError::new("unchanged_translation",true);continue;}
                            service.routing.lock().observe(&operation,&id,epoch,elapsed,!value.cached&&changed,false,&service.settings);
                            succeeded=true;if let Some(tx)=sender.take(){let _=tx.send(Ok(value));}
                        }
                        Err(error)=>{let genuine=!matches!(error.code.as_str(),"translation.cancelled"|"translation.configuration_changed"|"translation.storage"|"translation.circuit_open");service.routing.lock().observe(&operation,&id,epoch,elapsed,genuine,genuine,&service.settings);last=error;}
                    }
                }
            }}
            tasks.abort_all();while tasks.join_next().await.is_some(){}
            if succeeded&&last.code!="translation.cancelled"&&last.code!="translation.configuration_changed"&&last.code!="translation.deadline"{service.routing.lock().finish(&operation,epoch,&service.settings);}else{service.routing.lock().cancel(&operation);}
            service.requests.lock().remove(&request.request_id);service.request_operations.lock().remove(&request.request_id);
            if let Some(tx)=sender{let _=tx.send(Err(last));}
        });
        let result=receiver.await.unwrap_or_else(|_|Err(TranslationError::new("unavailable",true)));guard.armed=false;result
    }
    pub async fn test(&self,id:&str)->Result<ConnectionTest>{
        if id=="auto"{return Err(TranslationError::new("select_provider_for_test",false));}
        let cfg=self.config.read().clone();let request=TranslationRequest{request_id:format!("test-{}",uuid::Uuid::new_v4()),texts:vec!["Hello, world.".into()],source_language:"en".into(),target_language:Some("zh-CN".into()),expected_generation:Some(cfg.generation),operation_id:None,operation_final:None};
        // Tests intentionally bypass persistent result cache and hit the chosen provider.
        let p=cfg.providers.get(id).cloned().unwrap_or_default();let key=self.provider_key(id)?;
        let start=Instant::now();let text=tokio::time::timeout(Duration::from_millis(cfg.timeout_ms.saturating_mul(2)+1000),self.piece(id,&p,&key,&request.texts[0],"en","zh-CN",cfg.timeout_ms)).await.map_err(|_|TranslationError::new("timeout",true))??;
        if self.config.read().generation!=cfg.generation{return Err(TranslationError::new("configuration_changed",false));}
        Ok(ConnectionTest{provider:id.into(),text,elapsed_ms:start.elapsed().as_millis() as u64})
    }
}
/// UTF-8 byte limits are conservative for UTF-16/codepoint-limited APIs as well.
/// Prefer sentence/whitespace boundaries and retain every byte, including final tails.
pub fn split_text(text:&str,max:usize)->Vec<&str>{
    assert!(max>=4);let mut out=Vec::new();let mut rest=text;
    while rest.len()>max {let mut end=max;while !rest.is_char_boundary(end){end-=1;}
        if let Some((at,ch))=rest[..end].char_indices().rev().find(|(at,ch)|*at>=end/2 && (ch.is_whitespace()||matches!(ch,'.'|'!'|'?'|'。'|'！'|'？'|';'|'；'))){end=at+ch.len_utf8();}
        out.push(&rest[..end]);rest=&rest[end..];
    }
    if !rest.is_empty(){out.push(rest);}out
}
