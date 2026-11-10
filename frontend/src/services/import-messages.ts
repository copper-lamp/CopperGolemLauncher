import { useLocaleRef } from '../i18n';
const en = {
  upgrade: 'Upgrade', downgrade: 'Downgrade', reinstall: 'Reinstall same version', versionUnknown: 'Version order could not be determined; review both versions.',
  open: 'Import file', title: 'Confirm import', filename: 'Source file', version: 'Package version', installed: 'Installed version',
  instance: 'Target instance', choose: 'Choose an instance', noInstances: 'No instances are installed. Create an instance before importing this content.',
  retryInstances: 'Reload instances', permissions: 'Requested permissions', noPermissions: 'No additional permissions declared',
  unsigned: 'Publisher not verified', risk: 'This module may execute code with its declared permissions. Only continue if you trust its source.',
  acknowledge: 'I trust this source and accept the permissions and unverified publisher risk.',
  replace: 'I explicitly approve replacing the installed content or module.', replacement: 'An installed module already exists. Compare the versions before replacing it; importing an older version is a downgrade.',
  conflict: 'Existing content conflicts with this import. Review the details and explicitly approve replacement to retry.',
  worldCopy: 'This world will be imported as a new copy. Existing worlds are preserved.',
  commit: 'Import', cancel: 'Cancel', working: 'Importing…', picking: 'Opening file picker…', success: 'Import completed',
  loading: 'Loading instances…', next: 'Queued files', hash: 'SHA-256', unknown: 'Not supplied',
  pack: 'Resource / behavior pack', addon: 'Add-on', world: 'World', template: 'World template', module: 'Launcher module',
  drop: 'Drop files to inspect and confirm import', failed: 'Import failed. The ticket remains available for retry until it expires.',
};
const zh: Record<keyof typeof en, string> = {
  upgrade: '升级', downgrade: '降级', reinstall: '重新安装相同版本', versionUnknown: '无法确定版本先后，请检查两侧版本。',
  open: '导入文件', title: '确认导入', filename: '来源文件', version: '包版本', installed: '已安装版本',
  instance: '目标实例', choose: '请选择实例', noInstances: '尚未安装实例，请先创建实例再导入此内容。', retryInstances: '重新读取实例',
  permissions: '请求的权限', noPermissions: '未声明额外权限', unsigned: '发布者未验证',
  risk: '此模块可能按所声明的权限执行代码，请仅在信任来源时继续。', acknowledge: '我信任此来源，并接受所声明权限及发布者未验证的风险。',
  replace: '我明确同意替换已安装的内容或模块。', replacement: '已有同名模块。请比较版本后确认替换；导入更早版本会降级。',
  conflict: '此导入与已存在的内容冲突。请检查错误详情，明确同意替换后重试。', worldCopy: '世界将作为新副本导入，已有世界会保留。',
  commit: '导入', cancel: '取消', working: '正在导入…', picking: '正在打开文件选择器…', success: '导入完成', loading: '正在读取实例…',
  next: '排队文件', hash: 'SHA-256', unknown: '未提供', pack: '资源／行为包', addon: '附加包', world: '世界', template: '世界模板', module: '启动器模块',
  drop: '拖放文件以检查并确认导入', failed: '导入失败，票据过期前可在此重试。',
};
export function useImportText() {
  const locale = useLocaleRef();
  return (key: keyof typeof en) => (locale.value.toLowerCase().startsWith('zh') ? zh : en)[key];
}
