mod compiler;
mod lexer;
#[macro_use]
mod log;
mod parser;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::lexer::type_def::Span;

use crate::compiler::emitter::{EmitDriver, Lua53};
use crate::compiler::linter::{FileAnalysis, LintDriver, analyze_project};
use crate::parser::ast::Tree;
use crate::parser::parser::{NodeSyntax, Parser, SyntaxError};

fn parse(args: &[String]) -> HashMap<String, Vec<SyntaxError>> {
    // 按路径收集每个文件的语法诊断。parse() 返回的 Tree 借自每个文件的局部
    // Parser，无法跨迭代存进 map，故这里只留持有所权的 Vec<SyntaxError>
    let mut results: HashMap<String, Vec<SyntaxError>> = HashMap::new();
    for arg in args {
        parse_path(Path::new(arg), &mut results);
    }
    results
}

// 递归处理一个路径：文件直接分析，目录则遍历其下每个条目（含子目录）
fn parse_path(path: &Path, results: &mut HashMap<String, Vec<SyntaxError>>) {
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    parse_path(&entry.path(), results);
                }
            }
            Err(err) => log_error!("无法读取目录 {}：{}", path.display(), err),
        }
    } else if path.is_file() {
        let contents = std::fs::read_to_string(path).unwrap();
        let mut parser = parser::parser::Parser::new(&contents);
        let errors = parser.parse().errors;
        for e in &errors {
            log_error!(
                "{}: 语法错误 @ {}..{}：{}",
                path.display(),
                e.span.start,
                e.span.end,
                e.msg
            );
        }
        results.insert(path.display().to_string(), errors);
    } else {
        log_error!("路径不存在: {}", path.display());
    }
}

fn check(args: &[String]) {
    // 1) 收齐所有源（模块名 + 内容）。内容要全程留着：Tree 借 Parser、
    //    Parser 又借内容，prefill / run 两相位都要访问，中途不能释放
    let mut sources: Vec<(String, PathBuf, String)> = Vec::new();
    for arg in args {
        let path = Path::new(arg);
        // 目录以自身为根算模块名；单文件以其父目录为根（于是 a/b.tua -> "b"）
        let base = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        collect_sources(&base, path, &mut sources);
    }
    if sources.is_empty() {
        log_error!("没有找到 .tua 源文件");
        return;
    }

    // 2) 并行解析。每个文件一棵独立的 Tree、互不共享状态（Session 只在之后的
    //    lint 两相位才登场），所以这一步天然可并行：用 scoped 线程各自解析一份，
    //    只把 owned 的语法错误列表带回来。Tree 借着 Parser、跨不了线程边界，
    //    留到 join 之后按顺序回来取（parser.tree()），顺带把诊断顺序钉稳
    let mut parsers: Vec<Parser> = sources.iter().map(|(_, _, src)| Parser::new(src)).collect();
    let error_lists: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = parsers
            .iter_mut()
            .map(|parser| scope.spawn(move || parser.parse().errors))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut modules: Vec<(&str, &Tree<NodeSyntax>)> = Vec::new();
    for (((module, _, _), parser), errors) in sources.iter().zip(parsers.iter()).zip(&error_lists) {
        for e in errors {
            log_error!(
                "{module}: 语法错误 @ {}..{}：{}",
                e.span.start,
                e.span.end,
                e.msg
            );
        }
        modules.push((module.as_str(), parser.tree()));
    }

    // 3) 工程级两相位检查，诊断按模块取回
    let results = LintDriver::check_project(&modules);
    let mut total = 0usize;
    for (module, diags) in &results {
        for d in diags {
            total += 1;
            log_error!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
        }
    }
    if total == 0 {
        log_info!("检查通过：{} 个模块，无诊断", modules.len());
    } else {
        log_error!("检查失败：共 {total} 条诊断");
    }
}

/// 递归收集 `path` 下的 .tua 源：目录逐项深入，文件按扩展名过滤后读内容。
/// `base` 是算模块名的根（相对路径去扩展名、'/' 换 '.'）
fn collect_sources(base: &Path, path: &Path, out: &mut Vec<(String, PathBuf, String)>) {
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    collect_sources(base, &entry.path(), out);
                }
            }
            Err(err) => log_error!("无法读取目录 {}: {}", path.display(), err),
        }
    } else if path.is_file() {
        // check 只处理 .tua 源；目录里夹杂的别的文件跳过，免得当源去解析报错
        if path.extension().and_then(|e| e.to_str()) != Some("tua") {
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => out.push((module_name(base, path), path.to_path_buf(), contents)),
            Err(err) => log_error!("无法读取文件 {}: {}", path.display(), err),
        }
    } else {
        log_error!("路径不存在: {}", path.display());
    }
}

/// 由文件路径推模块名：相对 `base` 去掉 .tua 扩展、路径分隔符换成 '.'，
/// 与 `import {..} in "pkg.account"` 的写法对上（examples/pkg/account.tua -> pkg.account）
fn module_name(base: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(base).unwrap_or(path).with_extension("");
    rel.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join(".")
}

/// `compile`：解析 -> 类型检查 -> 发射。任一阶段有错都中止，绝不落地半成品代码。
/// 通过检查后，每个模块的 .tua 在原地生成同名 .lua（`with_extension`）
fn compile(args: &[String]) {
    // 1) 收源。和 check 一样内容全程留着（Tree 借 Parser、Parser 借内容）；
    //    compile 额外要留源路径，用来定位输出的 .lua
    let mut sources: Vec<(String, PathBuf, String)> = Vec::new();
    for arg in args {
        let path = Path::new(arg);
        let base = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        collect_sources(&base, path, &mut sources);
    }
    if sources.is_empty() {
        log_error!("没有找到 .tua 源文件");
        return;
    }

    // 2) 并行解析（同 check）：scoped 线程各解析一份，只带回 owned 的语法错误列表
    let mut parsers: Vec<Parser> = sources.iter().map(|(_, _, src)| Parser::new(src)).collect();
    let error_lists: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = parsers
            .iter_mut()
            .map(|parser| scope.spawn(move || parser.parse().errors))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut modules: Vec<(&str, &Tree<NodeSyntax>)> = Vec::new();
    let mut syntax_errors = 0usize;
    for (((module, _, _), parser), errors) in sources.iter().zip(parsers.iter()).zip(&error_lists) {
        for e in errors {
            log_error!(
                "{module}: 语法错误 @ {}..{}：{}",
                e.span.start,
                e.span.end,
                e.msg
            );
            syntax_errors += 1;
        }
        modules.push((module.as_str(), parser.tree()));
    }
    if syntax_errors > 0 {
        log_error!("compile 中止：{syntax_errors} 处语法错误，未生成代码");
        return;
    }

    // 3) 工程级两相位类型检查：有诊断就别发射，免得把已知错误的树落成 .lua
    let results = LintDriver::check_project(&modules);
    let mut total = 0usize;
    for (module, diags) in &results {
        for d in diags {
            total += 1;
            log_error!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
        }
    }
    if total > 0 {
        log_error!("compile 中止：共 {total} 条诊断，未生成代码");
        return;
    }

    // 4) 发射：每个模块原地 .tua -> .lua。EmitDriver 挂 Lua53 一个后端，
    //    以后要多目标（别的 Lua 版本 / 别的语言）再往 driver 上加
    let mut driver = EmitDriver::new();
    driver.add_emitter(Box::new(Lua53::new()));
    let mut emitted = 0usize;
    for ((_, path, _), &(module, tree)) in sources.iter().zip(&modules) {
        let target = path.with_extension("lua");
        driver.write(tree, module, &target.display().to_string());
        log_debug!("生成 {}", target.display());
        emitted += 1;
    }
    log_info!("compile 完成：{emitted} 个模块已生成 .lua");
}

/// `plugin`：给编辑器插件喂数据。解析每个 .tua，跑独立的 plugin_linter
/// （语义高亮 + 跳转定义，和类型检查隔离），把结果按文件序列化成一个 JSON
/// 数组打到 stdout。区间是字节偏移，行列换算交给插件那侧
fn plugin(args: &[String]) {
    // 收源，和 check / compile 同款：内容全程留着（Tree 借 Parser、Parser 借内容）
    let mut sources: Vec<(String, PathBuf, String)> = Vec::new();
    for arg in args {
        let path = Path::new(arg);
        let base = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        collect_sources(&base, path, &mut sources);
    }
    if sources.is_empty() {
        log_error!("没有找到 .tua 源文件");
        return;
    }

    // 并行解析（同 check）：这里带回的是每个文件的注释 span，高亮要用
    let mut parsers: Vec<Parser> = sources.iter().map(|(_, _, src)| Parser::new(src)).collect();
    let comment_lists: Vec<Vec<Span>> = std::thread::scope(|scope| {
        let handles: Vec<_> = parsers
            .iter_mut()
            .map(|parser| scope.spawn(move || parser.parse().comments))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let mut modules: Vec<(&str, &Tree<NodeSyntax>, Vec<Span>)> = Vec::new();
    for (((module, _, _), parser), comments) in
        sources.iter().zip(parsers.iter()).zip(&comment_lists)
    {
        modules.push((module.as_str(), parser.tree(), comments.clone()));
    }

    // 工程级分析：先建导出索引再逐文件出结果，顺序和 modules / sources 对齐
    let results = analyze_project(&modules);

    // 模块名 -> 源路径，给跨模块跳转补上目标文件路径
    let module_path: HashMap<&str, &Path> = sources
        .iter()
        .map(|(module, path, _)| (module.as_str(), path.as_path()))
        .collect();

    println!("{}", render_plugin_json(&sources, &results, &module_path));
}

/// 把分析结果拼成 JSON 数组。没有引第三方序列化库，这里手拼；只需转义字符串里的
/// 引号 / 反斜杠 / 控制符，路径与模块名都走这条
fn render_plugin_json(
    sources: &[(String, PathBuf, String)],
    results: &[FileAnalysis],
    module_path: &HashMap<&str, &Path>,
) -> String {
    let mut out = String::from("[");
    for (i, ((_, path, _), fa)) in sources.iter().zip(results.iter()).enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("\n  {");
        out.push_str(&format!("\"module\":\"{}\",", json_escape(&fa.module)));
        out.push_str(&format!(
            "\"path\":\"{}\",",
            json_escape(&path.display().to_string())
        ));

        // tokens
        out.push_str("\"tokens\":[");
        for (j, t) in fa.tokens.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"start\":{},\"end\":{},\"kind\":\"{}\"}}",
                t.span.start,
                t.span.end,
                t.kind.as_str()
            ));
        }
        out.push_str("],");

        // links
        out.push_str("\"links\":[");
        for (j, l) in fa.links.iter().enumerate() {
            if j > 0 {
                out.push(',');
            }
            let target_path = module_path
                .get(l.target_module.as_str())
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            out.push_str(&format!(
                "{{\"start\":{},\"end\":{},\"targetModule\":\"{}\",\"targetPath\":\"{}\",\"targetStart\":{},\"targetEnd\":{}}}",
                l.span.start,
                l.span.end,
                json_escape(&l.target_module),
                json_escape(&target_path),
                l.target.start,
                l.target.end
            ));
        }
        out.push_str("]}");
    }
    out.push_str("\n]");
    out
}

/// JSON 字符串最小转义
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    log::init_from_env();

    if let Some(cmd) = args.get(1) {
        // 跳过程序名与子命令，摘掉日志开关（-q/-v/--log=）后余下才是路径参数
        let rest = log::take_flags(&args[2..]);
        match cmd.as_str() {
            "compile" => compile(&rest),
            "parse" => {
                parse(&rest);
            }
            "check" => check(&rest),
            "plugin" => plugin(&rest),
            other => log_error!("未知命令: {other}"),
        }
    } else {
        log_error!("未提供命令");
    }
}
