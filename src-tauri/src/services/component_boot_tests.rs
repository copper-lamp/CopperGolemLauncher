use super::*;
fn isolated_service()->(Arc<ExternalComponentService>,Staging){
    let root=std::env::temp_dir().join(format!("copper-boot-test-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();
    let paths=Paths::with_root(root.clone());let db=Arc::new(super::super::database::DatabaseService::open(&root.join("test.sqlite")).unwrap());db.migrate_scope("core",super::super::database::CORE_MIGRATIONS).unwrap();
    let events=Arc::new(EventBus::new());let settings=Arc::new(SettingsService::new(db,events.clone(),Default::default()).unwrap());
    (Arc::new(ExternalComponentService::new(&paths,settings,events)),Staging(root))
}
#[tokio::test] async fn schedule_returns_without_waiting_and_runs_once(){
    let (service,_cleanup)=isolated_service();let start=std::time::Instant::now();
    service.spawn_background(&tokio::runtime::Handle::current());service.spawn_background(&tokio::runtime::Handle::current());
    assert!(start.elapsed()<Duration::from_millis(100));assert!(service.boot_started.load(Ordering::SeqCst));assert!(service.states.lock().unwrap().is_empty());
}
#[tokio::test] async fn cancels_on_setting_and_foreground_generation(){
    let (service,_cleanup)=isolated_service();service.control.background.store(true,Ordering::SeqCst);service.settings.set("components.auto_update_enabled",&false).unwrap();
    assert!(service.controlled(0,std::future::pending::<Result<(),KernelError>>()).await.is_err());
    service.settings.set("components.auto_update_enabled",&true).unwrap();service.cancel();
    assert!(service.controlled(0,std::future::pending::<Result<(),KernelError>>()).await.is_err());assert!(service.states.lock().unwrap().is_empty());
    service.settings.set("components.auto_update_enabled",&false).unwrap();
    assert!(service.controlled(1,async{tokio::time::sleep(Duration::from_millis(150)).await;Ok(())}).await.is_ok());
}
#[test] fn incompatible_release_is_visible_but_never_an_install_candidate(){
    let data=serde_json::json!([{"tag_name":"v0.40.0"},{"tag_name":"v0.41.0-beta"},{"tag_name":"v0.39.0","draft":true}]);
    assert_eq!(newest_incompatible_lip(&data,"0.37.0"),Some("0.40.0".into()));
    assert!(resolve_lip(&data,"win-x64").is_err());
}
