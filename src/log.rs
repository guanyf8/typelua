//! 极简分级日志：整个进程一个全局阈值，低于阈值的消息直接丢弃。没引第三方
//! log crate —— 这个 CLI 只要「按级别开关 + 写 stderr」，够用就好。
//!
//! 级别从低到高 Error < Warn < Info < Debug，阈值越高印得越多，默认 Info。
//! 用环境变量 `TYPELUA_LOG=error|warn|info|debug` 定基线，命令行 `-q`/`-v`/
//! `--log=LEVEL` 覆盖（优先级更高）。一律写 stderr —— 生成的 .lua 由各命令
//! 自己落盘，不走这里，所以日志开到多大都不会污染真正的产物。

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LogLevel {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl LogLevel {
    fn from_u8(v: u8) -> LogLevel {
        match v {
            0 => LogLevel::Error,
            1 => LogLevel::Warn,
            2 => LogLevel::Info,
            _ => LogLevel::Debug,
        }
    }

    /// 大小写不敏感地认名字，认不出返回 `None`
    pub fn parse(name: &str) -> Option<LogLevel> {
        match name.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => Some(LogLevel::Error),
            "warn" | "warning" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" | "trace" => Some(LogLevel::Debug),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

pub fn set_level(level: LogLevel) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

pub fn level() -> LogLevel {
    LogLevel::from_u8(LEVEL.load(Ordering::Relaxed))
}

/// 该级别当前是否会被印出来
pub fn enabled(level: LogLevel) -> bool {
    level <= self::level()
}

/// 宏的落地点：级别不够直接返回，够了才写 stderr。调用方用 `log_*!` 宏，
/// 别直接调这个
pub fn emit(level: LogLevel, args: std::fmt::Arguments<'_>) {
    if enabled(level) {
        eprintln!("[{}] {}", level.tag(), args);
    }
}

/// 读环境变量 `TYPELUA_LOG` 定基线阈值；没设或认不出就保持默认 Info
pub fn init_from_env() {
    if let Ok(v) = std::env::var("TYPELUA_LOG")
        && let Some(level) = LogLevel::parse(&v)
    {
        set_level(level);
    }
}

/// 从参数里摘出日志开关（`-q`/`--quiet`、`-v`/`--verbose`、`--log=LEVEL`），
/// 就地调阈值，返回剩下的「真参数」。命令行优先级高于环境变量
pub fn take_flags(args: &[String]) -> Vec<String> {
    let mut rest = Vec::with_capacity(args.len());
    for arg in args {
        match arg.as_str() {
            "-q" | "--quiet" => set_level(LogLevel::Warn),
            "-v" | "--verbose" => set_level(LogLevel::Debug),
            other => match other.strip_prefix("--log=") {
                Some(name) => match LogLevel::parse(name) {
                    Some(level) => set_level(level),
                    None => emit(LogLevel::Error, format_args!("无法识别的日志级别：{name}")),
                },
                None => rest.push(arg.clone()),
            },
        }
    }
    rest
}

/// `log_error!` / `log_warn!` / `log_info!` / `log_debug!`：和 `println!` 一样
/// 收格式串，前面挂对应级别。`#[macro_use] mod log;` 把它们带进 crate 根，
/// 全 crate 免 `use` 直接调
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::LogLevel::Error, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::LogLevel::Warn, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::LogLevel::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::log::emit($crate::log::LogLevel::Debug, format_args!($($arg)*)) };
}
