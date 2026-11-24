import { registerAddonI18n } from '../i18n';
export const ONBOARDING_NAMESPACE = 'onboarding';
registerAddonI18n(ONBOARDING_NAMESPACE, {
  'en-US': {
    'preview.action': 'Preview onboarding', 'preview.notice': 'Preview only. Progress is temporary; settings, imports and installation are disabled.', 'preview.close': 'Back to About',
    gameRoot: 'Game root', confirm: 'Schedule migration', cancel: 'Cancel', confirmStorage: 'Verify this destination. The migration will run on restart and preserve the source on failure.',
    title: 'Welcome to Copper Golem', next: 'Continue', back: 'Back', finish: 'Finish setup', exit: 'Exit application', retry: 'Retry', loading: 'Loading saved setup…', saveFailed: 'Setup could not be saved. Your current step has been kept.',
    features: 'Your modular Minecraft Bedrock launcher', 'features.body': 'Copper Golem is a modular Bedrock launcher. The pages below show the core workflow without screenshots.', 'features.install': 'Install Bedrock versions and keep each game instance isolated.', 'features.isolation': 'Manage separate instances with independent worlds, resources and mods.', 'features.content': 'Manage resource packs and mods for the selected instance.', 'features.injection': 'Use supported client injection features when the platform and game version allow them.',
    theme: 'Choose your appearance', 'theme.body': 'This choice applies to the whole launcher and can be changed later in Personalization.', dark: 'Dark', light: 'Light', auto: 'Follow system',
    progress: 'Setup progress', 'preview.brand': 'Copper Golem', 'preview.launch': 'Launch',
    'features.preview': 'One home for your instances', 'theme.preview': 'Your launcher, your colors', 'storage.preview': 'Know where your files live', 'game.preview': 'Choose a game version', 'content.preview': 'Discover content', 'launch.preview': 'Select an instance and play', 'modules.preview': 'Extend your launcher',
    storage: 'Choose a game directory', 'storage.body': 'Choose where game versions and the .download directory are stored. Migration is verified at the next restart; existing data is retained on failure.', chooseStorage: 'Choose game root', storagePending: 'Storage migration scheduled. Restart to apply it.',
    game: 'Download a game version', 'game.body': 'Open Game Downloads, select a version and create an instance. Downloads can be paused and resumed in the download center. Desktop and Android packages are filtered for your platform.',
    content: 'Add content to an instance', 'content.body': 'Content Downloads provides resource packs, behavior packs and supported mods. Select a compatible target instance before installing. Package requirements depend on the game and loader.',
    launch: 'Start a game', 'launch.body': 'Choose an instance, its resources and mods before launching. On the home page, click a world card in the default mode to enter that world quickly. Required genuine-account verification depends on your platform.', 'launch.actionsTitle': 'Before launch', 'launch.actions': 'Switch instances from the home page, then manage resources and mods in the instance controls. Sign in when the selected platform requires a genuine account.', 'launch.quickTitle': 'Quick start', 'launch.quick': 'Keep the home page in its default mode and click a world card to launch directly into that world.',
    modules: 'Extensions', 'modules.body': 'Install trusted community extensions only after reviewing their author and permissions.', 'modules.installTitle': 'Install community extensions', 'modules.installPath': 'Open Settings → Extensions to browse and install community extensions. Restart when the extension asks for it.', 'authTitle': 'Genuine account verification', 'auth.body': 'Windows requires the Microsoft/Xbox account that owns the game. Android requires a supported signed-in account or an imported official game package. Unsupported platforms keep verification actions disabled.',
    install: 'Install selected', installing: 'Installing…', noModules: 'No compatible approved modules are available. You can continue and install later.', installed: 'Installed; restart required', permissions: 'Permissions', author: 'Author', moduleFailed: 'Installation failed. Retry it or continue without this module.',
    briefs: 'What is new', dismiss: 'Got it', 'release.modules.title': 'External modules, connected', 'release.modules.body': 'Install trusted external modules from Settings, import local .cglm packages and load their pages after restarting. The host validates module packages and declared permissions.', storageError: 'Storage could not be loaded. Retry, or review it later in Settings.', detectError: 'Installed preferences could not be read. Retry or continue with your current settings.',
  },
  'zh-CN': {
    'preview.action': '预览新手引导', 'preview.notice': '当前为预览，进度仅临时保留，设置修改、导入与安装已禁用。', 'preview.close': '返回关于',
    gameRoot: '游戏根目录', confirm: '安排迁移', cancel: '取消', confirmStorage: '请核对目标目录。迁移将在重启后执行，失败时保留原数据。',
    title: '欢迎使用铜傀儡', next: '继续', back: '上一步', finish: '完成设置', exit: '退出应用', retry: '重试', loading: '正在读取引导进度…', saveFailed: '无法保存引导进度，已保留当前步骤。',
    features: '模块化的基岩版启动器', 'features.body': '铜傀儡是模块化的基岩版启动器。以下页面用纯文本介绍核心流程，不展示截图。', 'features.install': '安装基岩版游戏版本，并让每个游戏实例彼此隔离。', 'features.isolation': '为不同实例分别管理世界、资源包和模组。', 'features.content': '为当前选中的实例管理资源包与模组。', 'features.injection': '在平台和游戏版本支持时使用客户端注入功能。',
    theme: '选择外观', 'theme.body': '主题会应用到整个启动器，之后可在个性化中修改。', dark: '深色', light: '浅色', auto: '跟随系统',
    progress: '引导进度', 'preview.brand': '铜傀儡', 'preview.launch': '启动',
    'features.preview': '所有实例，一个主页', 'theme.preview': '让外观合你心意', 'storage.preview': '清楚掌握文件位置', 'game.preview': '发现游戏版本', 'content.preview': '发现更多内容', 'launch.preview': '选择实例，即刻开始', 'modules.preview': '自由扩展启动器',
    storage: '选择游戏目录', 'storage.body': '选择游戏版本与 .download 下载目录的存放位置。目录迁移将在下次重启时校验执行，失败时保留原数据。', chooseStorage: '选择游戏根目录', storagePending: '已安排目录迁移，重启后应用。',
    game: '下载游戏版本', 'game.body': '进入游戏下载，选择版本并创建实例。任务可以在下载中心暂停和续传。桌面与安卓安装包按当前平台筛选。',
    content: '为实例添加内容', 'content.body': '内容下载提供资源包、行为包及受支持的模组。安装前请选择兼容的目标实例；不同内容需要对应的游戏版本与加载器。',
    launch: '启动游戏', 'launch.body': '启动前选择实例，并按需配置该实例的资源包与模组。首页保持默认模式时，点击世界卡片即可快速进入对应世界。正版验证要求取决于所用平台。', 'launch.actionsTitle': '启动前管理', 'launch.actions': '在首页切换实例，并进入实例控制管理资源包和模组。所用平台要求时，请使用拥有正版游戏的账号登录。', 'launch.quickTitle': '快速进入世界', 'launch.quick': '首页保持默认模式，点击世界卡片即可直接启动并进入该世界。',
    modules: '扩展', 'modules.body': '安装社区扩展前，请先核对作者与请求的权限。', 'modules.installTitle': '安装社区扩展', 'modules.installPath': '进入“设置-扩展”浏览并安装社区扩展。若扩展提示需要重启，请按提示操作。', 'authTitle': '正版验证要求', 'auth.body': 'Windows 需使用拥有游戏的 Microsoft/Xbox 账号验证。Android 需使用受支持的已登录账号或导入官方游戏包。其他不支持的平台会禁用正版验证相关操作。',
    install: '安装所选模块', installing: '正在安装…', noModules: '暂无兼容且已审核的模块，可继续并稍后安装。', installed: '已安装，重启生效', permissions: '权限', author: '作者', moduleFailed: '安装失败，可重试或继续而不安装此模块。',
    briefs: '本次更新介绍', dismiss: '知道了', 'release.modules.title': '外部模块现已接通', 'release.modules.body': '在设置中安装可信外部模块或导入本地 .cglm 文件，重启后加载模块页面。宿主会校验模块包与声明权限。', storageError: '无法读取储存位置，可重试或稍后在设置中查看。', detectError: '无法读取已安装偏好，可重试或继续使用当前设置。',
  },
});
export const onboardingKey = (key: string) => `module.${ONBOARDING_NAMESPACE}.${key}`;
