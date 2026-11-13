//! Translation IPC. Credentials never leave the write-only key argument.
use std::{collections::HashMap,sync::Arc};
use tauri::State;
use crate::services::translation::{provider::{self,ProviderInfo},TranslationService,TranslationConfig,TranslationRequest,TranslationResponse,TranslationError,ConnectionTest};
#[tauri::command]
pub fn translation_routing(service:State<'_,Arc<TranslationService>>)->crate::services::translation::routing::Status{service.routing_status()}
#[tauri::command]
pub fn translation_providers()->Vec<ProviderInfo>{provider::registry()}
#[tauri::command]
pub fn translation_config(service:State<'_,Arc<TranslationService>>)->Result<TranslationConfig,TranslationError>{service.config()}
#[tauri::command]
pub fn translation_save_config(service:State<'_,Arc<TranslationService>>,config:TranslationConfig,keys:Option<HashMap<String,String>>)->Result<TranslationConfig,TranslationError>{service.save_config(config,keys.unwrap_or_default())}
#[tauri::command]
pub async fn translation_translate(service:State<'_,Arc<TranslationService>>,request:TranslationRequest)->Result<TranslationResponse,TranslationError>{service.translate(request).await}
#[tauri::command]
pub fn translation_cancel(service:State<'_,Arc<TranslationService>>,request_id:String)->bool{service.cancel(&request_id)}
#[tauri::command]
pub fn translation_clear_cache(service:State<'_,Arc<TranslationService>>)->Result<u64,TranslationError>{service.clear_cache()}
#[tauri::command]
pub async fn translation_test(service:State<'_,Arc<TranslationService>>,provider:String)->Result<ConnectionTest,TranslationError>{service.test(&provider).await}
