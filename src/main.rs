mod compiler;
mod lexer;
#[macro_use]
mod log;
mod parser;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::lexer::type_def::Span;

use crate::compiler::RUNTIME_MODULE;
use crate::compiler::emitter::{EmitCtx, EmitDriver, Lua53};
use crate::compiler::linter::{DiagLevel, LintDriver, LinterOutput};
use crate::compiler::runtime;
use crate::parser::ast::Tree;
use crate::parser::parser::{NodeSyntax, Parser, SyntaxError};

fn parse(args: &[String]) -> bool {
    // 按路径收集每个文件的语法诊断。parse() 返回的 Tree 借自每个文件的局部
    // Parser，无法跨迭代存进 map，故这里只留持有所权的 Vec<SyntaxError>
    let mut results: HashMap<String, Vec<SyntaxError>> = HashMap::new();
    let mut ok = true;
    for arg in args {
        ok &= parse_path(Path::new(arg), &mut results);
    }
    if results.is_empty() {
        log_error!("没有找到可解析的文件");
        return false;
    }
    // 读不了的文件和语法错误都算失败：CI 只看退出码，不再 grep 输出
    ok && results.values().all(|errors| errors.is_empty())
}

// 递归处理一个路径：文件直接分析，目录则遍历其下每个条目（含子目录）。
// 返回 false 表示有路径读不了（文件本身有语法错由调用方看 error_lists）
fn parse_path(path: &Path, results: &mut HashMap<String, Vec<SyntaxError>>) -> bool {
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut ok = true;
                for entry in entries.flatten() {
                    ok &= parse_path(&entry.path(), results);
                }
                ok
            }
            Err(err) => {
                log_error!("无法读取目录 {}：{}", path.display(), err);
                false
            }
        }
    } else if path.is_file() {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
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
                true
            }
            Err(err) => {
                log_error!("无法读取文件 {}：{}", path.display(), err);
                false
            }
        }
    } else {
        log_error!("路径不存在: {}", path.display());
        false
    }
}

fn check(args: &[String]) -> bool {
    let mut collect_ok = true;
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
        collect_ok &= collect_sources(&base, path, &mut sources);
    }
    if sources.is_empty() {
        log_error!("没有找到 .tua 源文件");
        return false;
    }
    if reject_runtime_module_name(&sources) {
        return false;
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
    let mut syntax_errors = 0usize;
    for (((module, _, _), parser), errors) in sources.iter().zip(parsers.iter()).zip(&error_lists) {
        for e in errors {
            syntax_errors += 1;
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
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for (module, diags) in &results {
        for d in diags {
            match d.severity {
                DiagLevel::Error => {
                    errors += 1;
                    log_error!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
                DiagLevel::Warning => {
                    warnings += 1;
                    log_warn!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
                DiagLevel::Info => {
                    log_info!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
            }
        }
    }
    // 语法错误和读不进来的路径也计入汇总：以前前者只打印不计数，后者连汇总都不进
    let total = errors + syntax_errors + usize::from(!collect_ok);
    if total == 0 {
        log_info!("检查通过：{} 个模块，{warnings} 条警告", modules.len());
        true
    } else {
        log_error!("检查失败：{total} 条错误，{warnings} 条警告");
        false
    }
}

/// 递归收集 `path` 下的 .tua 源：目录逐项深入，文件按扩展名过滤后读内容。
/// `base` 是算模块名的根（相对路径去扩展名、'/' 换 '.'）
fn collect_sources(base: &Path, path: &Path, out: &mut Vec<(String, PathBuf, String)>) -> bool {
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut ok = true;
                for entry in entries.flatten() {
                    ok &= collect_sources(base, &entry.path(), out);
                }
                ok
            }
            Err(err) => {
                log_error!("无法读取目录 {}: {}", path.display(), err);
                false
            }
        }
    } else if path.is_file() {
        // check 只处理 .tua 源；目录里夹杂的别的文件跳过，免得当源去解析报错
        if path.extension().and_then(|e| e.to_str()) != Some("tua") {
            return true;
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                out.push((module_name(base, path), path.to_path_buf(), contents));
                true
            }
            Err(err) => {
                log_error!("无法读取文件 {}: {}", path.display(), err);
                false
            }
        }
    } else {
        log_error!("路径不存在: {}", path.display());
        false
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

/// 模块名撞上共享运行时的文件名：`compile` 先写各模块、再写
/// `__typelua_runtime.lua`，同一条路径上的用户模块会被静默覆盖。模块名是从
/// 路径推出来的，源码里没有能挂诊断的 span，所以只能在收齐源之后中止
fn reject_runtime_module_name(sources: &[(String, PathBuf, String)]) -> bool {
    for (module, path, _) in sources {
        if module == RUNTIME_MODULE {
            log_error!(
                "模块名 {RUNTIME_MODULE} 是编译器保留的：{} 生成的 .lua 会被共享运行时的同名文件覆盖，请把这个源文件改名",
                path.display()
            );
            return true;
        }
    }
    false
}

/// `compile`：解析 -> 类型检查 -> 发射。任一阶段有错都中止，绝不落地半成品代码。
/// 通过检查后每个模块生成 .lua：默认在源 .tua 旁原地生成（`with_extension`）；
/// 给了 `--out <dir>`（`-o` 同义）则按模块名铺到该目录下（pkg.account ->
/// <dir>/pkg/account.lua），子目录按需创建
fn compile(args: &[String]) -> bool {
    let mut collect_ok = true;
    // 0) 摘掉 `--out <dir>`（`-o` / `--out=<dir>` 同义）。给了就把 .lua 按模块名
    //    铺到该目录下（pkg.account -> <dir>/pkg/account.lua）；不给则在源文件原地生成
    let mut out_dir: Option<PathBuf> = None;
    let mut paths: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(dir) = arg.strip_prefix("--out=") {
            out_dir = Some(PathBuf::from(dir));
        } else if arg.as_str() == "--out" || arg.as_str() == "-o" {
            match it.next() {
                Some(dir) => out_dir = Some(PathBuf::from(dir)),
                None => {
                    log_error!("--out 需要一个输出目录参数");
                    return false;
                }
            }
        } else {
            paths.push(arg.clone());
        }
    }

    // 1) 收源。和 check 一样内容全程留着（Tree 借 Parser、Parser 借内容）；
    //    compile 额外要留源路径，用来定位输出的 .lua。bases 记「模块名的根」：
    //    共享运行时要落到唯一那个根上，多个根时没法决定
    let mut sources: Vec<(String, PathBuf, String)> = Vec::new();
    let mut bases: Vec<PathBuf> = Vec::new();
    for arg in &paths {
        let path = Path::new(arg);
        let base = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        if !bases.contains(&base) {
            bases.push(base.clone());
        }
        collect_ok &= collect_sources(&base, path, &mut sources);
    }
    if sources.is_empty() {
        log_error!("没有找到 .tua 源文件");
        return false;
    }
    // 读不进来的源不能只报一半就发射：宁可一行 .lua 都不写
    if !collect_ok {
        log_error!("compile 中止：有源目录或源文件读不进来，未生成代码");
        return false;
    }
    if reject_runtime_module_name(&sources) {
        return false;
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
        return false;
    }

    // 3) 工程级两相位类型检查：有诊断就别发射，免得把已知错误的树落成 .lua。
    //    发射端只读树，import 改写与前置声明由它自己算，这里只取诊断
    let results = LintDriver::check_project(&modules);
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for (module, diags) in &results {
        for d in diags {
            match d.severity {
                DiagLevel::Error => {
                    errors += 1;
                    log_error!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
                DiagLevel::Warning => {
                    warnings += 1;
                    log_warn!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
                DiagLevel::Info => {
                    log_info!("{module}: {} @ {}..{}", d.msg, d.span.start, d.span.end);
                }
            }
        }
    }
    if errors > 0 {
        log_error!("compile 中止：{errors} 条错误，{warnings} 条警告，未生成代码");
        return false;
    }

    // 4) 发射：每个模块原地 .tua -> .lua。EmitDriver 挂 Lua53 一个后端，
    //    以后要多目标（别的 Lua 版本 / 别的语言）再往 driver 上加。
    //    多模块工程走共享模式：文件头注入运行时、pub 类注册、import 来的类取值。
    //    运行时的落地目录先算好：原地生成又分属多个根时根本无从决定，
    //    得在**写任何文件之前**中止，不能留一堆半成品
    let shared = sources.len() > 1;
    let runtime_dir = if shared {
        match &out_dir {
            Some(dir) => Some(dir.clone()),
            None if bases.len() == 1 => Some(bases[0].clone()),
            None => {
                log_error!(
                    "compile 中止：源文件分属多个目录，共享运行时的落地位置不唯一；请用 --out 指定输出目录"
                );
                return false;
            }
        }
    } else {
        None
    };
    let mut driver = EmitDriver::new();
    driver.add_emitter(Box::new(Lua53::new()));
    let mut emitted = 0usize;
    for ((_, path, _), &(module, tree)) in sources.iter().zip(&modules) {
        // --out 给了就按模块名铺进该目录（并按需建子目录），否则在源文件旁原地生成
        let target = match &out_dir {
            Some(dir) => {
                let mut t = dir.join(module.replace('.', std::path::MAIN_SEPARATOR_STR));
                t.set_extension("lua");
                if let Some(parent) = t.parent()
                    && let Err(err) = std::fs::create_dir_all(parent)
                {
                    log_error!("无法创建输出目录 {}: {}", parent.display(), err);
                    return false;
                }
                t
            }
            None => path.with_extension("lua"),
        };
        let ctx = EmitCtx { module, shared };
        // 写出失败（磁盘满 / 没权限）也算编译失败，不能让 CI 误判成功
        if !driver.write(tree, &ctx, &target.display().to_string()) {
            return false;
        }
        log_debug!("生成 {}", target.display());
        emitted += 1;
    }
    // 5) 共享运行时只需要一份，位置在 4) 开头就定下了
    if let Some(dir) = runtime_dir {
        let runtime_path = dir.join(format!("{RUNTIME_MODULE}.lua"));
        if let Err(err) = std::fs::create_dir_all(&dir) {
            log_error!("无法创建输出目录 {}: {}", dir.display(), err);
            return false;
        }
        if let Err(err) = std::fs::write(&runtime_path, runtime::SOURCE) {
            log_error!("无法写出共享运行时 {}: {}", runtime_path.display(), err);
            return false;
        }
        log_debug!("生成 {}", runtime_path.display());
    }
    let dest = out_dir
        .as_ref()
        .map(|d| format!("到 {}", d.display()))
        .unwrap_or_else(|| "（原地）".to_string());
    log_info!("compile 完成：{emitted} 个模块已生成 .lua {dest}");
    true
}

/// `plugin`：给编辑器插件喂数据。解析每个 .tua，由 LintDriver::output_project
/// 统一运行 highlight_linter 与 type_linter，输出高亮、跳转和诊断。
/// 区间是字节偏移，行列换算交给插件那侧
fn plugin(args: &[String]) -> bool {
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
        return false;
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

    let modules: Vec<(&str, &Tree<NodeSyntax>, &[Span])> = sources
        .iter()
        .zip(parsers.iter())
        .zip(&comment_lists)
        .map(|(((module, _, _), parser), comments)| {
            (module.as_str(), parser.tree(), comments.as_slice())
        })
        .collect();

    // 两个 linter 共走这个工程级入口；BUILD 屏障与逐文件输出都由 driver 负责。
    let lint_outputs = LintDriver::output_project(&modules);

    // 模块名 -> 源路径，给跨模块跳转补上目标文件路径
    let module_path: HashMap<&str, &Path> = sources
        .iter()
        .map(|(module, path, _)| (module.as_str(), path.as_path()))
        .collect();

    println!(
        "{}",
        render_plugin_json(&sources, &lint_outputs, &module_path)
    );
    // 有语法/类型错误也照常输出 JSON：诊断走 diagnostics 字段，编辑器靠它画波浪线。
    // plugin 的退出码只表示「这份 JSON 有没有产出」，别把编辑器刷成空白
    true
}

/// 把分析结果拼成 JSON 数组。没有引第三方序列化库，这里手拼；只需转义字符串里的
/// 引号 / 反斜杠 / 控制符，路径与模块名都走这条
fn render_plugin_json(
    sources: &[(String, PathBuf, String)],
    lint_results: &[(String, Vec<LinterOutput>)],
    module_path: &HashMap<&str, &Path>,
) -> String {
    let mut out = String::from("[");
    for ((_, path, _), (module, outputs)) in sources.iter().zip(lint_results.iter()) {
        if out.len() > 1 {
            out.push(',');
        }
        out.push_str("\n  {");
        out.push_str(&format!("\"module\":\"{}\",", json_escape(module)));
        out.push_str(&format!(
            "\"path\":\"{}\",",
            json_escape(&path.display().to_string())
        ));

        // tokens
        out.push_str("\"tokens\":[");
        let mut first = true;
        for output in outputs {
            for token in &output.tokens {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!(
                    "{{\"start\":{},\"end\":{},\"kind\":\"{}\"}}",
                    token.span.start,
                    token.span.end,
                    token.kind.as_str()
                ));
            }
        }
        out.push_str("],");

        // links
        out.push_str("\"links\":[");
        let mut first = true;
        for output in outputs {
            for link in &output.links {
                if !first {
                    out.push(',');
                }
                first = false;
                let target_path = module_path
                    .get(link.target_module.as_str())
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                out.push_str(&format!(
                    "{{\"start\":{},\"end\":{},\"targetModule\":\"{}\",\"targetPath\":\"{}\",\"targetStart\":{},\"targetEnd\":{}}}",
                    link.span.start,
                    link.span.end,
                    json_escape(&link.target_module),
                    json_escape(&target_path),
                    link.target.start,
                    link.target.end
                ));
            }
        }
        out.push_str("],");

        // diagnostics：同一 output_project 结果里按 linter 来源聚合。
        out.push_str("\"diagnostics\":[");
        let mut first = true;
        for output in outputs {
            for diagnostic in &output.diagnostics {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!(
                    "{{\"start\":{},\"end\":{},\"severity\":\"{}\",\"source\":\"{}\",\"message\":\"{}\"}}",
                    diagnostic.span.start,
                    diagnostic.span.end,
                    diagnostic.severity.as_str(),
                    json_escape(&output.source),
                    json_escape(&diagnostic.msg)
                ));
            }
        }
        out.push_str("],");

        // completions：highlight_linter 收集的补全候选（名字 + 种类），编辑器拿去
        // 做 Tab 代码提示。补全只从 highlight 来、诊断只从 type 来，但都走同一份
        // output_project，这里照旧按来源合并
        out.push_str("\"completions\":[");
        let mut first = true;
        for output in outputs {
            for completion in &output.completions {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&format!(
                    "{{\"label\":\"{}\",\"kind\":\"{}\"}}",
                    json_escape(&completion.label),
                    completion.kind.as_str()
                ));
            }
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

fn main() -> ExitCode {
    let args = std::env::args().collect::<Vec<_>>();
    log::init_from_env();

    let ok = if let Some(cmd) = args.get(1) {
        // 跳过程序名与子命令，摘掉日志开关（-q/-v/--log=）后余下才是路径参数
        let rest = log::take_flags(&args[2..]);
        match cmd.as_str() {
            "compile" => compile(&rest),
            "parse" => parse(&rest),
            "check" => check(&rest),
            "plugin" => plugin(&rest),
            other => {
                log_error!("未知命令: {other}");
                false
            }
        }
    } else {
        log_error!("未提供命令");
        false
    };
    // 0 只代表「这一步真的成功了」。CI 直接看退出码，不再 grep 输出文本
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
