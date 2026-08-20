


/// 输入是 `Literal::to_string()` 
/// 不是字符字面量（字符串 / 字节 / 数字 / 原始字符串）一律返回 `None`。
pub(super) fn char_literal_value(text: &str) -> Option<char> {
    let body = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let Some(escape) = body.strip_prefix('\\') else {
        //未转义：必须恰好一个字符。`'中'` 这种多字节的也算一个
        let mut chars = body.chars();
        let c = chars.next()?;
        return chars.next().is_none().then_some(c);
    };
    //用整串相等来匹配，长度检查就自带了：`'\nx'` 这种非法输入不会误判成 '\n'
    match escape {
        "'" => Some('\''),
        "\"" => Some('"'),
        "\\" => Some('\\'),
        "n" => Some('\n'),
        "t" => Some('\t'),
        "r" => Some('\r'),
        "0" => Some('\0'),
        //`'\xNN'`（Rust 只允许 \x00..=\x7F）与 `'\u{...}'`
        _ => {
            if let Some(hex) = escape.strip_prefix('x') {
                hex_to_char(hex)
            } else if let Some(hex) = escape
                .strip_prefix("u{")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                hex_to_char(hex)
            } else {
                None
            }
        }
    }
}

fn hex_to_char(hex: &str) -> Option<char> {
    let digits: String = hex.chars().filter(|c| *c != '_').collect();
    if digits.is_empty() {
        return None;
    }
    char::from_u32(u32::from_str_radix(&digits, 16).ok()?)
}