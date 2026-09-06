// 内置模块前端包统一注册入口。
//
// 各内置模块在此 import 自己的 `register.ts`，登记左导航入口与内容区路由；
// 内核经 `registry.ts` 统一注入 SideNav 与 Router。模块新增时在此追加一行 import，
// 无需改动内核 Shell。注册顺序即左导航顺序：开始页在最上方。
//
// 模块前端结构约定：页面组件 / API 封装 / 语言包集中在
// `frontend/src/modules/<id>/` 下，保持模块内聚、互不影响。

// 开始页模块前端包（接管 `/` 与 `/version-settings`）。
import "./home/register";

// 内容下载模块前端包。
import "./content-download/register";

// 其它内置模块（游戏下载）在各自开发过程中在此追加：
// import "./game-download/register";
