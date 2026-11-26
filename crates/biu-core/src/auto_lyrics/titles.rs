const DECORATION_PATTERN: &[&str] = &[
    "music video",
    "musicvideo",
    "full ver",
    "fullver",
    "hi-res",
    "1080p",
    "60fps",
    "cover",
    "hires",
    "720p",
    "live",
    "动态歌词",
    "完整版",
    "歌词版",
    "纯音乐",
    "高音质",
    "4k",
    "中字",
    "伴奏",
    "合集",
    "字幕",
    "官方",
    "无损",
    "现场",
    "电台",
    "翻唱",
    "联投",
];

const DECORATION_TOKENS: &[&str] = &[
    "music video",
    "musicvideo",
    "full ver",
    "fullver",
    "中日双语版",
    "高清修复版",
    "ai声线",
    "ai翻唱",
    "中文字幕",
    "中日双语",
    "中日歌词",
    "中英歌词",
    "剧情插曲",
    "动态歌词",
    "无损音质",
    "背景音乐",
    "高清修复",
    "高音质版",
    "bgm",
    "dj版",
    "中文版",
    "主题曲",
    "伴奏版",
    "修复版",
    "全景声",
    "剪辑版",
    "加长版",
    "双声道",
    "双语版",
    "变奏版",
    "合唱版",
    "合集版",
    "吉他版",
    "和声版",
    "女声版",
    "完整版",
    "官方版",
    "录音棚",
    "提取版",
    "无损版",
    "日文版",
    "歌词版",
    "游戏版",
    "片头曲",
    "片尾曲",
    "环绕版",
    "现场版",
    "电台版",
    "男声版",
    "百万级",
    "纯享版",
    "纯音乐",
    "试听版",
    "钢琴版",
    "附歌词",
    "高音质",
    "dj",
    "专注",
    "串烧",
    "伴奏",
    "剪辑",
    "副歌",
    "加速",
    "助眠",
    "双语",
    "合唱",
    "合集",
    "吉他",
    "和声",
    "女声",
    "学习",
    "慢速",
    "插曲",
    "无损",
    "杜比",
    "歌词",
    "母带",
    "消音",
    "混剪",
    "游戏",
    "片段",
    "环绕",
    "现场",
    "电台",
    "男声",
    "纯享",
    "试听",
    "超清",
    "车载",
    "连唱",
    "钢琴",
    "高清",
];

const GENERIC_CANDIDATE_PATTERN: &[&str] = &[
    "instrumental",
    "original",
    "version",
    "audio",
    "boss战",
    "intro",
    "outro",
    "remix",
    "theme",
    "track",
    "video",
    "boss",
    "demo",
    "feat",
    "from",
    "full",
    "part",
    "song",
    "that",
    "this",
    "viii",
    "with",
    "your",
    "and",
    "bgm",
    "for",
    "iii",
    "ost",
    "our",
    "the",
    "ver",
    "vii",
    "you",
    "主题曲",
    "加长版",
    "变奏版",
    "完整版",
    "战斗曲",
    "片头曲",
    "片尾曲",
    "现场版",
    "纯音乐",
    "ed",
    "ep",
    "ft",
    "ii",
    "iv",
    "ix",
    "me",
    "mv",
    "my",
    "op",
    "pt",
    "pv",
    "vi",
    "中字",
    "主题",
    "伴奏",
    "决战",
    "前奏",
    "剧情",
    "剪辑",
    "动画",
    "原唱",
    "双语",
    "变奏",
    "合集",
    "国语",
    "场景",
    "字幕",
    "官方",
    "尾声",
    "庆典",
    "序曲",
    "序章",
    "战斗",
    "插曲",
    "日语",
    "歌词",
    "游戏",
    "片段",
    "电台",
    "纪念",
    "终章",
    "翻唱",
    "英语",
    "试听",
    "鉴赏",
    "间奏",
    "音乐",
    "预告",
    "i",
    "v",
    "x",
];

const ALBUM: &[&str] = &[
    "ost",
    "soundtrack",
    "原声带",
    "原声",
    "专辑",
    "精选集",
    "音乐集",
];
const SEPARATORS: &[char] = &[
    '-', '–', '—', '－', '_', '|', '｜', '│', '┃', '/', '／', '·', '・', '~', '～', '&', '＆', '、',
];

fn matching_bracket(ch: char) -> Option<char> {
    Some(match ch {
        '《' => '》',
        '〈' => '〉',
        '【' => '】',
        '「' => '」',
        '『' => '』',
        '“' => '”',
        '"' => '"',
        '（' => '）',
        '(' => ')',
        '[' => ']',
        '〔' => '〕',
        '［' => '］',
        '｛' => '｝',
        _ => return None,
    })
}

fn quoted(title: &str) -> impl Iterator<Item = (&str, usize, usize)> {
    let mut last_end = 0;
    title.char_indices().filter_map(move |(start, left)| {
        if start < last_end {
            return None;
        }
        let right = matching_bracket(left)?;
        let inner = start + left.len_utf8();
        let end = inner + title[inner..].find(right)?;
        last_end = end + right.len_utf8();
        Some((title[inner..end].trim(), start, last_end))
    })
}

fn is_decoration(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    DECORATION_PATTERN.iter().any(|token| lower.contains(token))
        || lower
            .strip_prefix('p')
            .is_some_and(|value| value.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        || lower.contains('第')
            && lower.contains(|c: char| c.is_ascii_digit())
            && lower.contains(['集', '话', '期'])
}

fn is_hidden_bracket(left: char) -> bool {
    matches!(left, '【' | '[' | '（' | '(' | '〔' | '［' | '｛')
}

pub fn clean_track_title(title: &str) -> String {
    let mut cleaned = String::new();
    let mut last_end = 0;
    for (inner, start, end) in quoted(title) {
        cleaned.push_str(&title[last_end..start]);
        cleaned.push(' ');
        if !is_hidden_bracket(title[start..].chars().next().unwrap()) || !is_decoration(inner) {
            cleaned.push_str(inner);
        }
        cleaned.push(' ');
        last_end = end;
    }
    cleaned.push_str(&title[last_end..]);
    let mut output = cleaned;
    let mut cleaned = output.to_ascii_lowercase();
    for token in DECORATION_TOKENS {
        while let Some(index) = cleaned.find(token) {
            let end = index + token.len();
            output.replace_range(index..end, &" ".repeat(token.len()));
            cleaned.replace_range(index..end, &" ".repeat(token.len()));
        }
    }
    let mut topic = false;
    let output = output
        .chars()
        .filter(|ch| {
            if ch.is_whitespace() {
                topic = false;
            }
            if matches!(ch, '#' | '＃') {
                topic = true;
            }
            !topic
        })
        .collect::<String>();
    output
        .replace(['_', '|', '｜'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn normalize(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '！'..='～' => char::from_u32(ch as u32 - 0xfee0).unwrap(),
            'ς' => 'σ',
            _ => ch,
        })
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .replace('ß', "ss")
}

pub fn build_title_candidates(title: &str) -> Vec<String> {
    let spans = quoted(title).collect::<Vec<_>>();
    let fingerprints = spans
        .iter()
        .filter_map(|(inner, _, _)| {
            let mut key = normalize(inner);
            let mut is_album = false;
            for token in ALBUM {
                if key.contains(token) {
                    key = key.replace(token, "");
                    is_album = true;
                }
            }
            let fingerprint: String = key
                .chars()
                .take_while(|ch| ch.is_ascii_lowercase() || ('\u{4e00}'..='\u{9fa5}').contains(ch))
                .take(4)
                .collect();
            (is_album && fingerprint.chars().count() >= 2).then_some(fingerprint)
        })
        .collect::<Vec<_>>();

    let mut candidates = Vec::new();
    let mut push = |text: &str| {
        let text = text.trim();
        let key = normalize(text);
        if key.is_empty()
            || is_decoration(text)
            || GENERIC_CANDIDATE_PATTERN.contains(&text.to_ascii_lowercase().as_str())
            || text
                .chars()
                .all(|ch| ch.is_ascii_digit() || ".-_".contains(ch))
            || fingerprints.iter().any(|prefix| {
                prefix == &key || key.chars().count() >= 2 && prefix.starts_with(&key)
            })
            || candidates.iter().any(|candidate| candidate == text)
        {
            return;
        }
        candidates.push(text.to_owned());
    };
    fn segments(source: &str, push: &mut impl FnMut(&str)) {
        let cleaned = clean_track_title(source);
        let words = cleaned.split(SEPARATORS).flat_map(str::split_whitespace);
        let mut latin = String::new();
        for word in words {
            let latin_word = word.bytes().any(|ch| ch.is_ascii_alphabetic())
                && word
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"'.".contains(&ch));
            if latin_word {
                if !latin.is_empty() {
                    latin.push(' ');
                }
                latin.push_str(word);
            } else {
                if latin.chars().count() >= 2 {
                    push(&latin);
                }
                latin.clear();
                if word.chars().count() >= 2 {
                    push(word);
                }
            }
        }
        if latin.chars().count() >= 2 {
            push(&latin);
        }
        cleaned.split(SEPARATORS).for_each(&mut *push);
        push(&cleaned);
    }

    if let Some((_, _, end)) = spans.iter().find(|(inner, _, _)| {
        ALBUM
            .iter()
            .any(|token| inner.to_ascii_lowercase().contains(token))
    }) {
        segments(&title[*end..], &mut push);
    }
    spans.iter().for_each(|(inner, _, _)| push(inner));
    segments(title, &mut push);
    candidates
}

pub fn build_search_queries(title: &str) -> Vec<String> {
    build_title_candidates(title)
        .into_iter()
        .filter(|text| text.encode_utf16().count() <= 28)
        .take(6)
        .collect()
}

pub(super) fn has_word(text: &str, word: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token.eq_ignore_ascii_case(word))
}

pub(super) fn strong_match(title: &str, text: &str) -> bool {
    let key = normalize(title);
    if key.is_empty() {
        return false;
    }
    if title.is_ascii() {
        let tokens = |text: &str| {
            text.split(|ch: char| !ch.is_ascii_alphanumeric())
                .filter(|s| !s.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        };
        let title_words = tokens(title);
        if title_words.is_empty() {
            return false;
        }
        let text_words = tokens(text);
        return text_words
            .windows(title_words.len())
            .any(|words| words == title_words);
    }
    if key.chars().count() < 3 {
        return quoted(text).any(|(fragment, _, _)| normalize(fragment) == key)
            || text
                .split(|ch: char| {
                    ch.is_whitespace()
                        || SEPARATORS.contains(&ch)
                        || matching_bracket(ch).is_some()
                        || "》〉】」』”）)]：:".contains(ch)
                })
                .any(|fragment| normalize(fragment) == key);
    }
    normalize(text).contains(&key)
}

pub(super) fn without_trailing_note(title: &str) -> &str {
    let title = title.trim();
    if !title.ends_with([')', '）']) {
        return title;
    }
    let mut depth = 0u32;
    for (index, ch) in title.char_indices().rev() {
        match ch {
            ')' | '）' => depth += 1,
            '(' | '（' => {
                depth -= 1;
                if depth == 0 && index != 0 {
                    return title[..index].trim_end();
                }
            }
            _ => {}
        }
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_candidates_prioritize_quotes_and_skip_album_names() {
        assert_eq!(
            build_search_queries("中日双语版《雨爱》老头子携影分身#合唱")[0],
            "雨爱"
        );
        assert_eq!(
            build_search_queries("【高音质】《鸣潮 3.2 OST》空の箱 - Girls Band Cry")[0],
            "空の箱"
        );
        assert!(
            build_title_candidates("【1080p】Girls Band Cry - 片段")
                .contains(&"Girls Band Cry".into())
        );
        assert_eq!(
            without_trailing_note("視界の隅 朽ちる音 (新川崎(仮))"),
            "視界の隅 朽ちる音"
        );
    }
}
