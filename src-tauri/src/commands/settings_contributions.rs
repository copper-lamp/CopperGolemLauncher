//! Declarative module settings schemas. Owners are rechecked on every access.
#[cfg(test)] mod tests {
 use super::*;
 #[test] fn schemas_enforce_namespace_type_range_and_disposal(){let owner="schema-test";unregister(owner,"*");let section=|title:&str|serde_json::from_value::<Section>(serde_json::json!({"id":"panel","category":"general","titleKey":title,"fields":[{"id":"count","titleKey":"example.count","type":"number","default":2,"min":1,"max":4}]})).unwrap();assert!(register(owner,"example",section("other.title")).is_err());register(owner,"example",section("example.title")).unwrap();let item=field(owner,"panel.count").unwrap();assert!(validate(&item,&serde_json::json!(3)).is_ok());assert!(validate(&item,&serde_json::json!(9)).is_err());assert!(validate(&item,&serde_json::json!("3")).is_err());assert!(field("different-owner","panel.count").is_err());assert!(register(owner,"example",section("example.title")).is_err());unregister(owner,"*");assert!(field(owner,"panel.count").is_err());}
}
use std::{collections::HashMap,sync::{Mutex,OnceLock}};
use serde::Deserialize;use serde_json::Value;use crate::error::KernelError;
#[derive(Clone,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Field{pub id:String,pub title_key:String,#[serde(rename="type")]pub kind:String,pub default:Value,pub min:Option<f64>,pub max:Option<f64>,pub options:Option<Vec<Choice>>}
#[derive(Clone,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Choice{pub value:String,pub title_key:String}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
pub struct Section{pub id:String,pub category:String,pub title_key:String,pub fields:Vec<Field>}
type Schemas=HashMap<String,HashMap<String,Field>>;
fn schemas()-> &'static Mutex<Schemas>{static S:OnceLock<Mutex<Schemas>>=OnceLock::new();S.get_or_init(||Mutex::new(HashMap::new()))}
fn lexical(s:&str)->bool{!s.is_empty()&&s.len()<=64&&s.chars().all(|c|c.is_ascii_alphanumeric()||matches!(c,'_'|'-'))}
fn invalid()->KernelError{KernelError::InvalidArgument("Invalid module settings schema or value".into())}
pub fn validate(field:&Field,value:&Value)->Result<(),KernelError>{
 let valid=match field.kind.as_str(){"boolean"=>value.is_boolean(),"string"=>value.as_str().is_some_and(|s|s.len()<=4096),"number"=>value.as_f64().is_some_and(|n|n.is_finite()&&field.min.is_none_or(|min|n>=min)&&field.max.is_none_or(|max|n<=max)),"enum"=>value.as_str().is_some_and(|s|field.options.as_ref().is_some_and(|opts|opts.iter().any(|o|o.value==s))),_=>false};if valid{Ok(())}else{Err(invalid())}
}
pub fn register(owner:&str,namespace:&str,section:Section)->Result<(),KernelError>{
 if !lexical(&section.id)||!matches!(section.category.as_str(),"general"|"appearance"|"storage"|"download"|"launch"|"translation"|"modules"|"components"|"privacy"|"update"|"about")||!section.title_key.starts_with(&format!("{namespace}."))||section.fields.len()>64{return Err(invalid())}
 let mut fields=HashMap::new();for field in section.fields{if !lexical(&field.id)||!field.title_key.starts_with(&format!("{namespace}."))||field.min.is_some_and(|n|!n.is_finite())||field.max.is_some_and(|n|!n.is_finite())||matches!((field.min,field.max),(Some(a),Some(b))if a>b){return Err(invalid())}if field.kind=="enum"{let options=field.options.as_ref().ok_or_else(invalid)?;if options.is_empty()||options.len()>64||options.iter().any(|o|o.value.len()>4096||!o.title_key.starts_with(&format!("{namespace}."))){return Err(invalid())}}validate(&field,&field.default)?;let key=format!("{}.{}",section.id,field.id);if fields.insert(key,field).is_some(){return Err(invalid())}}
 let mut all=schemas().lock().map_err(|_|invalid())?;let own=all.entry(owner.to_owned()).or_default();if own.keys().any(|k|k.starts_with(&format!("{}.",section.id))){return Err(invalid())}if own.len()+fields.len()>512{return Err(invalid())}own.extend(fields);Ok(())
}
pub fn field(owner:&str,key:&str)->Result<Field,KernelError>{schemas().lock().map_err(|_|invalid())?.get(owner).and_then(|s|s.get(key)).cloned().ok_or_else(invalid)}
pub fn unregister(owner:&str,section:&str){if let Ok(mut all)=schemas().lock(){if section=="*"{all.remove(owner);return;}if let Some(own)=all.get_mut(owner){own.retain(|key,_|!key.starts_with(&format!("{section}.")));}}}
