//! Passive routing: only user operations create measurements; no probes or source text persisted.
use super::*;
const TTL:i64=14*86400;
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(default)]
struct Metric {id:String,successes:u32,failures:u32,total_ms:u64}
#[derive(Clone,Default,Serialize,Deserialize)]
#[serde(default)]
struct Snapshot {generation:u64,expires_at:i64,operations:u32,selected:Vec<String>,providers:BTreeMap<String,Metric>,epoch:u64}
#[derive(Serialize)]
pub struct Status {phase:&'static str,operations:u32,required_operations:u32,selected:Vec<String>,expires_at:i64,providers:Vec<Timing>}
#[derive(Serialize)]
struct Timing {id:String,successes:u32,failures:u32,mean_ms:Option<u64>}
struct Observation {ms:u64,network:bool,failed:bool}
struct Operation {generation:u64,epoch:u64,signature:String,ids:Vec<String>,pending:usize,final_seen:bool,started:Instant,observations:BTreeMap<String,Observation>}
pub struct Routing {snapshot:Snapshot,operations:HashMap<String,Operation>,retired:HashMap<String,Instant>}
impl Routing {
 pub fn load(settings:&SettingsService,generation:u64)->Self{let mut s=Self{snapshot:settings.get_or("translation.routing.v1",Snapshot::default()),operations:HashMap::new(),retired:HashMap::new()};s.ensure(generation);s}
 fn ensure(&mut self,generation:u64){if self.snapshot.generation!=generation||self.snapshot.expires_at<=now(){self.clear(generation);}self.operations.retain(|_,op|op.started.elapsed()<Duration::from_secs(600));self.retired.retain(|_,at|at.elapsed()<Duration::from_secs(600));}
 pub fn clear(&mut self,generation:u64){let epoch=self.snapshot.epoch.wrapping_add(1);self.snapshot=Snapshot{generation,expires_at:now()+TTL,epoch,..Default::default()};for id in self.operations.keys(){self.retired.insert(id.clone(),Instant::now());}self.operations.clear();}
 pub fn status(&mut self,generation:u64)->Status{self.ensure(generation);Status{phase:if self.snapshot.selected.is_empty(){"learning"}else{"selected"},operations:self.snapshot.operations,required_operations:10,selected:self.snapshot.selected.clone(),expires_at:self.snapshot.expires_at,providers:self.snapshot.providers.values().map(|p|Timing{id:p.id.clone(),successes:p.successes,failures:p.failures,mean_ms:if p.successes>0{Some(p.total_ms/u64::from(p.successes))}else{None}}).collect()}}
 pub fn begin(&mut self,id:&str,signature:String,generation:u64,available:Vec<String>,final_batch:bool)->Result<(Vec<String>,u64)>{
  self.ensure(generation);if self.retired.contains_key(id){return Err(TranslationError::new("operation_finished",false));}if let Some(op)=self.operations.get_mut(id){if op.signature!=signature||op.generation!=generation{return Err(TranslationError::new("operation_changed",false));}if op.final_seen{return Err(TranslationError::new("operation_finished",false));}op.pending+=1;op.final_seen=final_batch;return Ok((op.ids.clone(),op.epoch));}
  if self.operations.len()>=128{return Err(TranslationError::new("too_many_requests",true));}
  let ids=if self.snapshot.selected.is_empty(){available}else{self.snapshot.selected.iter().filter(|id|available.contains(id)).cloned().collect()};if ids.is_empty(){return Err(TranslationError::new("missing_provider_configuration",false));}
  let epoch=self.snapshot.epoch;self.operations.insert(id.into(),Operation{generation,epoch,signature,ids:ids.clone(),pending:1,final_seen:final_batch,started:Instant::now(),observations:BTreeMap::new()});Ok((ids,epoch))
 }
 pub fn cancel(&mut self,id:&str){self.operations.remove(id);self.retired.insert(id.into(),Instant::now());}
 #[cfg(test)] pub fn select_for_test(&mut self,ids:Vec<String>){self.snapshot.selected=ids;}
 pub fn observe(&mut self,operation:&str,id:&str,epoch:u64,elapsed:u64,network:bool,failed:bool,settings:&SettingsService){
  if self.snapshot.epoch!=epoch||!self.operations.contains_key(operation){return;}if failed&&self.snapshot.selected.iter().any(|candidate|candidate==id){let generation=self.snapshot.generation;self.clear(generation);let _=settings.set("translation.routing.v1",&self.snapshot);return;}
  let Some(op)=self.operations.get_mut(operation) else{return;};if op.epoch!=epoch{return;}let entry=op.observations.entry(id.into()).or_insert(Observation{ms:0,network:false,failed:false});entry.ms=entry.ms.saturating_add(elapsed);entry.network|=network;entry.failed|=failed;
 }
 pub fn finish(&mut self,operation:&str,epoch:u64,settings:&SettingsService){
  let Some(op)=self.operations.get_mut(operation) else{return;};if op.epoch!=epoch{return;}op.pending=op.pending.saturating_sub(1);if op.pending!=0||!op.final_seen{return;}
  let Some(op)=self.operations.remove(operation) else{return;};self.retired.insert(operation.into(),Instant::now());if self.snapshot.epoch!=epoch{return;}let mut sampled=false;
  for(id,value)in op.observations{if !value.network{continue;}sampled=true;let p=self.snapshot.providers.entry(id.clone()).or_insert(Metric{id,..Default::default()});if value.failed{p.failures=p.failures.saturating_add(1);}else{p.successes=p.successes.saturating_add(1);p.total_ms=p.total_ms.saturating_add(value.ms);}}
  if sampled&&self.snapshot.operations<10{self.snapshot.operations+=1;}
  if self.snapshot.operations>=10&&self.snapshot.selected.is_empty(){let mut ranked:Vec<_>=self.snapshot.providers.values().filter(|p|p.successes>0).collect();ranked.sort_by(|a,b|{let left=u64::from(a.failures)*u64::from(b.successes+b.failures);let right=u64::from(b.failures)*u64::from(a.successes+a.failures);left.cmp(&right).then_with(||(a.total_ms/u64::from(a.successes)).cmp(&(b.total_ms/u64::from(b.successes)))).then_with(||a.id.cmp(&b.id))});self.snapshot.selected=ranked.into_iter().take(2).map(|p|p.id.clone()).collect();self.snapshot.expires_at=now()+TTL;}
  let _=settings.set("translation.routing.v1",&self.snapshot);
 }
}
