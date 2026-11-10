use super::*;
fn isolated_service()->(Arc<ExternalComponentService>,Staging){
    let root=std::env::temp_dir().join(format!("copper-boot-test-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();
    let paths=Paths::with_root(root.clone());let db=Arc::new(super::super::database::DatabaseService::open(&root.join("test.sqlite")).unwrap());db.migrate_scope("core",super::super::database::CORE_MIGRATIONS).unwrap();
    let events=Arc::new(EventBus::new());let settings=Arc::new(SettingsService::new(db,events.clone(),Default::default()).unwrap());
    (Arc::new(ExternalComponentService::new(&paths,settings,events)),Staging(root))
}
/// Wall-clock evidence for the real scheduler, without touching installed components or network.
/// A current-thread runtime makes an accidental blocking sleep observable by the heartbeat.
#[tokio::test(flavor="current_thread")]
async fn schedule_returns_without_waiting_and_runs_once(){
    let runtime=tokio::runtime::Handle::current();
    let baseline=runtime.metrics().num_alive_tasks();
    let samples:Vec<_>=(0..3).map(|_|isolated_service()).collect();
    let start=std::time::Instant::now();
    for (sample,(service,_)) in samples.iter().enumerate(){
        let call_start=std::time::Instant::now();
        service.spawn_background(&runtime);service.spawn_background(&runtime);
        let elapsed=call_start.elapsed();
        println!("scheduler sample={sample} two_calls_us={}",elapsed.as_micros());
        assert!(elapsed<Duration::from_millis(100));
        assert!(service.boot_started.load(Ordering::SeqCst));
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
    let heartbeat=start.elapsed();
    println!("scheduler heartbeat_ms={:.3} alive={}",heartbeat.as_secs_f64()*1000.0,runtime.metrics().num_alive_tasks()-baseline);
    assert!(heartbeat<Duration::from_millis(250));
    assert_eq!(runtime.metrics().num_alive_tasks(),baseline+3,"duplicate calls must not create duplicate jobs");
    tokio::time::sleep_until(tokio::time::Instant::from_std(start+Duration::from_secs(19))).await;
    assert_eq!(runtime.metrics().num_alive_tasks(),baseline+3,"jobs must remain delayed before 20 seconds");
    println!("scheduler delayed_at_ms={:.3}",start.elapsed().as_secs_f64()*1000.0);
    tokio::time::timeout(Duration::from_secs(4),async {
        while runtime.metrics().num_alive_tasks()!=baseline {tokio::time::sleep(Duration::from_millis(10)).await;}
    }).await.expect("empty isolated services must finish after the delay without network");
    println!("scheduler all_finished_ms={:.3}",start.elapsed().as_secs_f64()*1000.0);
    assert!(start.elapsed()>=Duration::from_secs(20));
    for (service,_) in &samples {service.spawn_background(&runtime);}
    tokio::task::yield_now().await;
    assert_eq!(runtime.metrics().num_alive_tasks(),baseline,"completed jobs must not restart");
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
