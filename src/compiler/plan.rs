//! 发射计划：linter 算出来、emitter 照着改写的少量位置。
//!
//! 只装两类事实，且都只关乎 class 的**值位**：
//!   - import 来的类名要换成 `__typelua.get(module, class)`：那张类表在对面
//!     模块里，本文件没有它可以捕获的名字
//!   - 同文件的前向**值**引用要提到文件头 `local` 里：Lua 的 `local` 只作用于
//!     它后面的语句。类表在文件头就直接建好（`local B = {}`），声明处只往里
//!     填 —— 早先的字面量（函数体、`__index`）抓到的都是同一张表
//!
//! 键用**节点下标**而不是 Span：计划由一次解析产出，也只喂给同一次解析的树，
//! 所以下标是稳的；而 Span 还得从产生式里反推名字 token 的范围，多一层易错。
//! 具体键是「emitter 会走到、并且要整段换掉的那个节点」：值位是 @VarRef 本身，
//! 继承是 @ClassDeclExtends 的 extendtype（children[5]）。
//!
//! 计划是纯优化/落地信息，不影响类型检查结论：拿不到计划（单文件、测试）时
//! emitter 按空计划走，与 master 的输出一致。

use std::collections::HashMap;

/// 共享运行时在每个模块里占的局域名。用户占用它时 linter 直接报错 ——
/// 注入的 `local __typelua = require(…)` 会和用户的值互相覆盖
pub const RUNTIME_LOCAL: &str = "__typelua";

/// 运行时模块的 require 名，也是 `--out` 目录里那个文件名
pub const RUNTIME_MODULE: &str = "__typelua_runtime";

/// 一个 import 来的类：运行时去哪张表取它。
/// `class` 存的是**导出时的原名**，不是本文件的别名 —— 别名只影响本文件的
/// 类型命名空间，注册表里的键是原名
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedClass {
    pub module: String,
    pub class: String,
}

/// 一个模块的发射计划
#[derive(Debug, Default, Clone)]
pub struct ModulePlan {
    /// 值位节点下标 -> 改写目标。值位是 @VarRef，继承是 extendtype 节点
    pub rewrites: HashMap<usize, ImportedClass>,
    /// 需要在文件头建表（`local 名字 = {}`）的本地类名，按声明位置排序。
    /// 和 import 改写的区别：这里仍是本文件的变量，只是提前占位
    pub predeclares: Vec<String>,
}

impl ModulePlan {
    pub fn rewrite_at(&self, node: usize) -> Option<&ImportedClass> {
        self.rewrites.get(&node)
    }

    pub fn is_predeclared(&self, name: &str) -> bool {
        self.predeclares.iter().any(|n| n == name)
    }
}

/// 整个工程的计划。键是模块名（`import {..} in "pkg.account"` 里那个）
#[derive(Debug, Default, Clone)]
pub struct ProjectPlan {
    modules: HashMap<String, ModulePlan>,
}

impl ProjectPlan {
    pub fn module(&self, name: &str) -> Option<&ModulePlan> {
        self.modules.get(name)
    }

    pub fn insert(&mut self, name: String, plan: ModulePlan) {
        self.modules.insert(name, plan);
    }
}
