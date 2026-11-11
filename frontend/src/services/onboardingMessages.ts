import { registerAddonI18n } from '../i18n';
export const ONBOARDING_NAMESPACE = 'onboarding';
registerAddonI18n(ONBOARDING_NAMESPACE, {
  'en-US': {
    chooseLauncherStorage: 'Choose launcher data root',
    aliasRequired: 'Choose a shorter alias using the manual binding form below.',
    launcherRoot: 'Launcher data root', gameRoot: 'Game root', confirm: 'Schedule migration', cancel: 'Cancel', confirmStorage: 'Verify this destination. The migration will run on restart and preserve the source on failure.',
    title: 'Welcome to Copper Golem', next: 'Continue', back: 'Back', finish: 'Finish setup', exit: 'Exit application', retry: 'Retry', loading: 'Loading saved setup…', saveFailed: 'Setup could not be saved. Your current step has been kept.',
    features: 'Your modular Minecraft Bedrock launcher', 'features.body': 'Manage instances, download game versions and discover content. Built-in modules share one theme and settings system; optional modules can be installed later.',
    theme: 'Choose your appearance', 'theme.body': 'This choice applies to the whole launcher and can be changed later in Personalization.', dark: 'Dark', light: 'Light', auto: 'Follow system',
    levi: 'LeviLauncher instances detected', 'levi.body': 'Review compatible instances before binding. Your existing instances stay in place; binding does not copy them.', bind: 'Bind selected instance', bound: 'Bound', unavailable: 'This capability is unavailable on this platform or in the current backend.',
    storage: 'Choose where your files live', 'storage.body': 'The launcher stores settings and caches in its own data root. A game root contains versions and the .download download directory. Migration is verified at the next restart; existing data is retained on failure.', chooseStorage: 'Choose game root', storagePending: 'Storage migration scheduled. Restart to apply it.',
    game: 'Download a game version', 'game.body': 'Open Game Downloads, select a version and create an instance. Downloads can be paused and resumed in the download center. Desktop and Android packages are filtered for your platform.',
    content: 'Add content to an instance', 'content.body': 'Content Downloads provides resource packs, behavior packs and supported mods. Select a compatible target instance before installing. Package requirements depend on the game and loader.',
    launch: 'Launch your instance', 'launch.body': 'Select an instance on the start page, review its launch settings and sign in when required. Missing prerequisites are shown before launch. Platform capabilities differ and unsupported actions remain disabled.',
    modules: 'Optional external modules', 'modules.body': 'Nothing is selected by default. Review the author and requested permissions before installing third-party code. Installation takes effect after restarting. Offline? Continue without installing and return in Settings later.',
    install: 'Install selected', installing: 'Installing…', noModules: 'No compatible approved modules are available. You can continue and install later.', installed: 'Installed; restart required', permissions: 'Permissions', author: 'Author', moduleFailed: 'Installation failed. Retry it or continue without this module.',
    briefs: 'What is new', dismiss: 'Got it', 'release.modules.title': 'External modules, connected', 'release.modules.body': 'Install trusted external modules from Settings, import local .cglm packages and load their pages after restarting. The host validates module packages and declared permissions.', storageError: 'Storage could not be loaded. Retry, or review it later in Settings.', detectError: 'Instance detection failed. Retry detection or continue without binding.',
  },
  'zh-CN': {
    chooseLauncherStorage: '选择启动器数据根',
    aliasRequired: '请使用下方手动关联表单选择较短别名。',
    launcherRoot: '启动器数据根', gameRoot: '游戏根目录', confirm: '安排迁移', cancel: '取消', confirmStorage: '请核对目标目录。迁移将在重启后执行，失败时保留原数据。',
    title: '欢迎使用铜傀儡', next: '继续', back: '上一步', finish: '完成设置', exit: '退出应用', retry: '重试', loading: '正在读取引导进度…', saveFailed: '无法保存引导进度，已保留当前步骤。',
    features: '模块化的基岩版启动器', 'features.body': '管理实例、下载游戏版本并发现内容。内置模块共享主题和设置体系；可选模块可以稍后安装。',
    theme: '选择外观', 'theme.body': '主题会应用到整个启动器，之后可在个性化中修改。', dark: '深色', light: '浅色', auto: '跟随系统',
    levi: '发现 LeviLauncher 实例', 'levi.body': '绑定前请检查兼容实例。原实例保留在原位置，绑定不会复制实例。', bind: '绑定所选实例', bound: '已绑定', unavailable: '当前平台或后端尚不支持此能力。',
    storage: '了解文件保存位置', 'storage.body': '启动器数据根保存设置与缓存。游戏根下包含 versions 和 .download 下载目录。目录迁移将在下次重启时校验执行，失败时保留原数据。', chooseStorage: '选择游戏根目录', storagePending: '已安排目录迁移，重启后应用。',
    game: '下载游戏版本', 'game.body': '进入游戏下载，选择版本并创建实例。任务可以在下载中心暂停和续传。桌面与安卓安装包按当前平台筛选。',
    content: '为实例添加内容', 'content.body': '内容下载提供资源包、行为包及受支持的模组。安装前请选择兼容的目标实例；不同内容需要对应的游戏版本与加载器。',
    launch: '启动实例', 'launch.body': '在开始页选择实例，检查启动设置并按需登录。启动前会展示缺少的前置组件。各平台能力有所不同，未支持的操作会明确禁用。',
    modules: '选择可选外部模块', 'modules.body': '默认不选择任何模块。安装第三方代码前请检查作者与请求的权限，安装后重启生效。离线时可直接继续，之后在设置中安装。',
    install: '安装所选模块', installing: '正在安装…', noModules: '暂无兼容且已审核的模块，可继续并稍后安装。', installed: '已安装，重启生效', permissions: '权限', author: '作者', moduleFailed: '安装失败，可重试或继续而不安装此模块。',
    briefs: '本次更新介绍', dismiss: '知道了', 'release.modules.title': '外部模块现已接通', 'release.modules.body': '在设置中安装可信外部模块或导入本地 .cglm 文件，重启后加载模块页面。宿主会校验模块包与声明权限。', storageError: '无法读取储存位置，可重试或稍后在设置中查看。', detectError: '实例检测失败，可重试检测或继续而不绑定。',
  },
});
export const onboardingKey = (key: string) => `module.${ONBOARDING_NAMESPACE}.${key}`;
