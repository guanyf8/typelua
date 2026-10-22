//! 宿主的窄接口：源码进、Lua 文本出，不经过 linter。
//!
//! 语法错误转成 `行:列: 消息`，真实路径由调用方补 —— loader 手里有文件名，
//! 编辑器插件手里是缓冲区名，同一个函数不用为两边各写一套。

use crate::compiler::emitter::emit_source;
use crate::parser::parser::SyntaxError;

/// 解析并发射一份 `.tua`。类型检查不参与，语法错误原样返回。
///
/// 总是按多模块工程发射（共享运行时头 + pub 类登记）：loader 加载的模块
/// 天然可能互相 import；单文件模式下 `import` 本来也不允许。
pub fn compile_source(module: &str, source: &str) -> Result<String, Vec<String>> {
    emit_source(module, source, true).map_err(|errors| {
        errors
            .iter()
            .map(|error| format_error(source, error))
            .collect()
    })
}

fn format_error(source: &str, error: &SyntaxError) -> String {
    let (line, column) = line_col(source, error.span.start);
    format!("{line}:{column}: {}", error.msg)
}

/// 字节偏移 -> 1 基的行列。列按字符数算，终端里一个 `é` 不会算成两列。
///
/// 偏移来自 lexer，正常落在字符边界上；越界或落在多字节字符中间时向内收，
/// 报错的位置可能差一列，但不会 panic。
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut end = offset.min(source.len());
    while end > 0 && !source.is_char_boundary(end) {
        end -= 1;
    }
    let bytes = &source.as_bytes()[..end];
    let line = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
    let line_start = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let column = source[line_start..end].chars().count() + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_without_running_the_checker() {
        // 类型不匹配只有 checker 会报；发射端只擦掉注解
        let lua = compile_source("m", "local x:number = \"s\"\n").unwrap();
        assert!(lua.contains("local x = \"s\""));
    }

    #[test]
    fn shared_header_is_always_emitted() {
        let lua =
            compile_source("pkg.account", "pub class Account { balance:number = 0 }").unwrap();
        assert!(lua.starts_with(
            "local __typelua = require(\"__typelua_runtime\")\n__typelua.enter(\"pkg.account\")"
        ));
        assert!(lua.contains("__typelua.def(\"pkg.account\", \"Account\", Account)"));
    }

    #[test]
    fn syntax_errors_carry_line_and_column() {
        let errors = compile_source("m", "class A {\n  x:number = 1\n").unwrap_err();
        assert!(!errors.is_empty());
        assert!(
            errors.iter().all(|e| {
                let mut parts = e.splitn(3, ':');
                let line = parts.next().unwrap().parse::<usize>().is_ok();
                let col = parts.next().unwrap().parse::<usize>().is_ok();
                line && col
            }),
            "{errors:?}"
        );
    }

    #[test]
    fn line_col_counts_newlines_and_unicode_columns() {
        assert_eq!(line_col("a\nbc", 2), (2, 1));
        assert_eq!(line_col("a\nbc", 4), (2, 3));
        assert_eq!(line_col("é\nx", 3), (2, 1));
        // 偏移落在 é 的中间：向内收到字符边界
        assert_eq!(line_col("é\nx", 1), (1, 1));
        // 越界收尾
        assert_eq!(line_col("a\n", 99), (2, 1));
    }
}
