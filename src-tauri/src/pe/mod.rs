//! PE 读写（内核侧）：解析、导入表改写、整文件重排、校验和。
//!
//! # 用途
//!
//! 启动游戏前需要给 `Minecraft.Windows.exe` 的导入表加一条我们自己的 hook DLL，
//! 让 DLL 在游戏进程启动时被加载（见 `docs/启动链路与实例隔离.md`）。这是
//! **唯一**能让隔离与加载器生效的途径：`SHGetKnownFolderPath` 不读环境变量，
//! 注册 AppX 也不产生任何路径重定向。
//!
//! # 与 LeviLauncher 的关系
//!
//! 算法参考 `libs/LeviLauncher/internal/peeditor`（Go，GPL-3.0-only）并改写，
//! 来源见 `THIRD_PARTY_NOTICES`。相对参考实现的优化见
//! `docs/启动链路与实例隔离.md` §2.5，核心是三点：
//!
//! 1. **布局计算与文件写入分离**（[`Image::plan_add_import`] 只算不写，
//!    [`ImportPlan::apply`] 才落盘）——节表满、文件头扩张这类边界用例可以
//!    在不碰文件系统的前提下测实。
//! 2. **写入是单次 uniform 位移**：文件头扩张时只在旧文件头之后插入若干字节，
//!    其余内容整体右移，所有「文件偏移」字段统一加上同一个 delta。参考实现
//!    逐段搬移并逐字段修补，容易漏掉符号表 / 调试目录 / 证书表这类
//!    非 RVA 字段。
//! 3. **幂等与体检**：重复注入不写文件；镜像若已被第三方改过（出现非本方的
//!    追加节），拒绝注入而不是硬改。

pub mod checksum;
mod import;
mod parse;
mod rebuild;
#[cfg(test)]
mod tests;

use std::fmt;

pub use import::ImportPlan;
pub use parse::{Image, Section};

/// PE 相关错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeError {
    /// 文件不是 PE 或结构被截断。
    NotPe(&'static str),
    /// 只支持 PE32+（x64）。x86 / ARM64 游戏包不在支持范围内。
    NotPe32Plus,
    /// 结构存在但数值不可用（如节地址与文件大小不一致）。
    Malformed(&'static str),
    /// 节表已满，无法追加新节。
    NoSectionRoom,
    /// 扩张后的文件头会与已有数据重叠。
    HeaderOverlap,
    /// 目标 DLL 已在导入表中（幂等命中，不是错误）。
    AlreadyImported(String),
    /// 镜像已被第三方改写（本方追加节缺失但存在其它异常），拒绝注入。
    ForeignPatch,
    /// 请求的导入项名非法（空串 / 超长）。
    BadImportName(&'static str),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPe(why) => write!(f, "不是有效的 PE 文件：{why}"),
            Self::NotPe32Plus => write!(f, "只支持 x64（PE32+）可执行文件"),
            Self::Malformed(why) => write!(f, "PE 结构异常：{why}"),
            Self::NoSectionRoom => write!(f, "节表已满，无法追加节"),
            Self::HeaderOverlap => write!(f, "扩张后的文件头与已有数据重叠"),
            Self::AlreadyImported(name) => write!(f, "{name} 已在导入表中"),
            Self::ForeignPatch => write!(
                f,
                "该可执行文件已被其它工具改写（存在非本方的追加节），拒绝注入"
            ),
            Self::BadImportName(why) => write!(f, "导入项名非法：{why}"),
        }
    }
}

impl std::error::Error for PeError {}

/// 本方注入追加的节名（8 字节内）。
pub const HOOK_SECTION_NAME: &str = ".copperh";

/// 解析并校验一份 PE32+ 镜像。
pub fn parse(data: Vec<u8>) -> Result<Image, PeError> {
    Image::parse(data)
}

/// 判断镜像是否已导入指定 DLL（大小写不敏感）。
pub fn dll_is_imported(data: &[u8], dll: &str) -> Result<bool, PeError> {
    Image::parse(data.to_vec())?.dll_is_imported(dll)
}
