; 卸载启动器时清理由启动器保管的第三方客户端。
;
; 客户端文件存放在 <app_data>\data\clients（见 src-tauri/src/modules/home/client.rs），
; 不属于任何游戏实例。卸载启动器时只删除这个目录，用户设置、实例与日志保持不动。
;
; 注意：NSIS 安装包之外的分发形态（MSI / 便携版）没有这段钩子；客户端目录仍会
; 随启动器数据目录一起由用户手动清理。该限制写在 docs/第三方客户端注入与收录.md。
!macro NSIS_HOOK_PREUNINSTALL
  RMDir /r "$APPDATA\${BUNDLEID}\data\clients"
!macroend
