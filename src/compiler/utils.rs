//! 编译层的纯工具函数：只依赖 lexer 交出来的原文，不认识类型系统、AST 和 Session。
//!
//! 判据就是这一条 —— 一个函数要进这里，它的签名里不能出现 `TypeId` / `Session` /
//! `Tree` 这些编译层的内部结构，否则它其实是某一层的方法，摊到公共模块只会把
//! 依赖方向倒过来。

/// 字符串字面量原文 -> 它表示的字符串。lexer 给的 `Token::STRING` 是**含定界符的原文**
/// （`make_token` 里 `&input[start..end]`，end 已经过了闭合符），所以两件事都要在这里
/// 做完：剥定界符、还原转义。不还原的话 `"\97"` 和 `"a"` 会 intern 成两个不同的字段名，
/// 而它们本是同一个键。
///
/// 定界符有两种：短引号 `'x'` / `"x"` 两侧各一个字符；长括号 `[[x]]` / `[==[x]==]`
/// 两侧各 n+2 个字符，n 是 '=' 的个数。长括号也走 `AcceptType::STRING`
/// （state_machine 的 LCLOSE），一律按一个字符剥会把 `[[a]]` 剥成 `[a]`。
/// 长括号里没有转义，且紧跟开括号的第一个换行按 Lua 的规矩吃掉。
///
/// 一处已知的不精确：`\ddd` / `\xXX` 在 Lua 里给的是**字节**，这里落进 Rust 的 String
/// （UTF-8）。小于 0x80 完全准确，大于等于 0x80 按 Latin-1 映成码点 —— 要做到字节精确
/// 得先有一个按字节存的 interner。将来字符串**值**也要参与拼接和输出时得回头解决，
/// 到那时值位和名字位共用这一份
use crate::lexer::type_def::Span;
use crate::parser::ast::Tree;
use crate::parser::parser::*;

pub struct Diagnostic {
    pub span: Span,
    pub msg: String,
}

/// 遍历访问者。`C` 是随遍历一路带下去的上下文（per-file 的临时状态），
/// 由驱动在遍历开始前建好、贯穿 prepare / enter / leave / finish 四个钩子 ——
/// 「像 ast 一样」当参数传，而不是长在访问者自己身上。utils 不认识 `C` 具体
/// 是什么（那会把类型系统的内部结构倒灌进公共模块），只把它透传
pub trait DFS<C> {
    /// 换文件了。per-file 的状态在这里清。`module` 是当前文件所属模块名，
    /// 跨模块的 pub / import 靠它认「谁导出的、往哪儿导」——只关心类型阶段的
    /// linter 会用到，其余的忽略即可
    fn prepare(
        &mut self,
        _data: &mut C,
        _ast: &Tree<NodeSyntax<'_>>,
        _module: &str,
        _diags: &mut Vec<Diagnostic>,
    ) {
    }

    /// 前序：子节点还没走。只适合「进作用域」这类必须先于子树的动作
    fn enter(
        &mut self,
        _data: &mut C,
        _ast: &Tree<NodeSyntax<'_>>,
        _node: usize,
        _diags: &mut Vec<Diagnostic>,
    ) {
    }

    /// 后序：子节点都走完了。综合属性的求值该在这里
    fn leave(
        &mut self,
        _data: &mut C,
        _ast: &Tree<NodeSyntax<'_>>,
        _node: usize,
        _diags: &mut Vec<Diagnostic>,
    ) {
    }

    /// 整棵树走完。跨节点攒起来的诊断（未初始化、未使用之类）在这里收口
    fn finish(&mut self, _data: &mut C, _diags: &mut Vec<Diagnostic>) {}
}

/// 通用的非递归 DFS：前序 enter、后序 leave，驱动任意 `DFS` 访问者走一棵树。
/// 从 linter 里抽出来放这儿，emitter 等别的遍历器照用同一套遍历，不必各写一遍栓。
/// warm_up / prepare / finish 这些「阶段」钩子由各自的驱动去调，这里只管纯遍历。
/// `V: ?Sized` 是为了能收 `&mut dyn DFS`（驱动拿的是 `Box<dyn DFS>`）
pub fn walk<C, V: DFS<C> + ?Sized>(
    visitor: &mut V,
    _data: &mut C,
    ast: &Tree<NodeSyntax<'_>>,
    root: usize,
    diags: &mut Vec<Diagnostic>,
) {
    //   Enter: 刚进入节点，还没处理子节点
    //   Leave: 所有子节点已处理完，即将离开
    enum Phase {
        Enter,
        Leave,
    }
    let mut stack: Vec<(usize, Phase)> = vec![(root, Phase::Enter)];

    while let Some((node, phase)) = stack.pop() {
        match phase {
            Phase::Enter => {
                visitor.enter(_data, ast, node, diags);

                // 准备处理子节点
                let children = ast.get_node(node).children.clone();
                if children.is_empty() {
                    // 没有子节点，直接 leave
                    visitor.leave(_data, ast, node, diags);
                } else {
                    // 先压 (node, Leave)，再逆序压子节点：栓是 LIFO，逆序才能按原顺序遍历
                    stack.push((node, Phase::Leave));
                    for child in children.into_iter().rev() {
                        stack.push((child, Phase::Enter));
                    }
                }
            }
            Phase::Leave => visitor.leave(_data, ast, node, diags),
        }
    }
}

pub fn string_literal(raw: &str) -> String {
    let b = raw.as_bytes();
    if b.first() == Some(&b'[') {
        // 开头是 '[' + n 个 '=' + '['，闭合对称
        let n = b[1..].iter().take_while(|&&c| c == b'=').count();
        let inner = &raw[n + 2..raw.len() - (n + 2)];
        return inner
            .strip_prefix("\r\n")
            .or_else(|| inner.strip_prefix('\n'))
            .unwrap_or(inner)
            .to_string();
    }
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut it = inner.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = it.next() else { break };
        match e {
            'a' => out.push('\u{07}'),
            'b' => out.push('\u{08}'),
            'f' => out.push('\u{0c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\u{0b}'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            // 反斜杠接一个真换行：值里就是一个换行
            '\n' => out.push('\n'),
            // `\z` 吃掉后面连续的空白
            'z' => {
                while it.peek().is_some_and(|c| c.is_whitespace()) {
                    it.next();
                }
            }
            // `\xXX`：两位十六进制，给的是一个字节
            'x' => {
                let mut v = 0u32;
                for _ in 0..2 {
                    match it.peek().and_then(|c| c.to_digit(16)) {
                        Some(d) => {
                            v = v * 16 + d;
                            it.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            // `\u{XXX}`：这个才是真码点，不是字节
            'u' => {
                if it.peek() == Some(&'{') {
                    it.next();
                    let mut v = 0u32;
                    while let Some(d) = it.peek().and_then(|c| c.to_digit(16)) {
                        v = v * 16 + d;
                        it.next();
                    }
                    if it.peek() == Some(&'}') {
                        it.next();
                    }
                    out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
                }
            }
            // `\ddd`：最多三位十进制，同样是一个字节
            '0'..='9' => {
                let mut v = e.to_digit(10).unwrap();
                for _ in 0..2 {
                    match it.peek().and_then(|c| c.to_digit(10)) {
                        Some(d) => {
                            v = v * 10 + d;
                            it.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            // 其余是非法转义，Lua 会报错。这里保守留原字符，报错归词法层
            other => out.push(other),
        }
    }
    out
}
