pub(super) fn contains_kana(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, 'ぁ'..='ゖ' | 'ァ'..='ヶ'))
}

fn hiragana(ch: char) -> char {
    if ('ァ'..='ヶ').contains(&ch) {
        char::from_u32(ch as u32 - 0x60).unwrap()
    } else {
        ch
    }
}

fn kana(ch: char) -> Option<&'static str> {
    const SYLLABLES: &[&str] = &[
        "a", "a", "i", "i", "u", "u", "e", "e", "o", "o", "ka", "ga", "ki", "gi", "ku", "gu", "ke",
        "ge", "ko", "go", "sa", "za", "shi", "ji", "su", "zu", "se", "ze", "so", "zo", "ta", "da",
        "chi", "ji", "", "tsu", "zu", "te", "de", "to", "do", "na", "ni", "nu", "ne", "no", "ha",
        "ba", "pa", "hi", "bi", "pi", "fu", "bu", "pu", "he", "be", "pe", "ho", "bo", "po", "ma",
        "mi", "mu", "me", "mo", "ya", "ya", "yu", "yu", "yo", "yo", "ra", "ri", "ru", "re", "ro",
        "wa", "wa", "i", "e", "o", "n", "vu", "ka", "ke",
    ];
    let index = (ch as u32).checked_sub('ぁ' as u32)?;
    SYLLABLES.get(index as usize).copied()
}

fn pair(first: char, second: char) -> Option<(&'static str, char)> {
    let vowel = match second {
        'ゃ' | 'ぁ' | 'ゎ' => 'a',
        'ぃ' => 'i',
        'ゅ' | 'ぅ' => 'u',
        'ぇ' => 'e',
        'ょ' | 'ぉ' => 'o',
        _ => return None,
    };
    let yoon = matches!(second, 'ゃ' | 'ゅ' | 'ょ');
    let stem = match (first, second) {
        ('き', _) if yoon => "ky",
        ('ぎ', _) if yoon => "gy",
        ('し', _) if yoon || second == 'ぇ' => "sh",
        ('じ' | 'ぢ', _) if yoon || second == 'ぇ' => "j",
        ('ち', _) if yoon || second == 'ぇ' => "ch",
        ('に', _) if yoon => "ny",
        ('ひ', _) if yoon => "hy",
        ('び', _) if yoon => "by",
        ('ぴ', _) if yoon => "py",
        ('み', _) if yoon => "my",
        ('り', _) if yoon => "ry",
        ('ふ', 'ゅ') => "fy",
        ('ふ', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ') => "f",
        ('ゔ', 'ゅ') => "vy",
        ('ゔ', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ') => "v",
        ('て', 'ぃ') => "t",
        ('て', 'ゅ') => "ty",
        ('で', 'ぃ') => "d",
        ('で', 'ゅ') => "dy",
        ('と', 'ぅ') => "t",
        ('ど', 'ぅ') => "d",
        ('つ', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ') => "ts",
        ('す', 'ぃ') => "s",
        ('ず', 'ぃ') => "z",
        ('う', 'ぃ' | 'ぇ' | 'ぉ') => "w",
        ('い', 'ぇ') => "y",
        ('く', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ' | 'ゎ') => "kw",
        ('ぐ', 'ぁ' | 'ぃ' | 'ぇ' | 'ぉ' | 'ゎ') => "gw",
        _ => return None,
    };
    Some((stem, vowel))
}

/// Convert kana to Hepburn romaji. Other text, including kanji, passes through unchanged.
pub fn kana_to_romaji(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut geminate = false;
    let mut last_vowel = None;
    while let Some(raw) = chars.next() {
        let ch = hiragana(raw);
        let next = chars.peek().copied().map(hiragana);
        match ch {
            'っ' => {
                if geminate {
                    output.push_str("tsu");
                }
                geminate = true;
                continue;
            }
            'ー' => {
                if let Some(vowel) = last_vowel {
                    output.push(vowel);
                } else {
                    output.push(raw);
                }
                continue;
            }
            'ん' => {
                if geminate {
                    output.push_str("tsu");
                    geminate = false;
                }
                let onset = next
                    .and_then(kana)
                    .and_then(|syllable| syllable.chars().next());
                output.push_str(match onset {
                    Some('b' | 'm' | 'p') => "m",
                    Some('a' | 'e' | 'i' | 'o' | 'u' | 'y') => "n'",
                    _ => "n",
                });
                last_vowel = None;
                continue;
            }
            _ => {}
        }
        let combined = next.and_then(|next| pair(ch, next));
        let (syllable, vowel) = if let Some((stem, vowel)) = combined {
            chars.next();
            (Some(stem), Some(vowel))
        } else {
            (kana(ch), None)
        };
        if let Some(syllable) = syllable {
            if geminate {
                match syllable.chars().next() {
                    Some('c') => output.push('t'),
                    Some('a' | 'e' | 'i' | 'o' | 'u') | None => output.push_str("tsu"),
                    Some(consonant) => output.push(consonant),
                }
            }
            output.push_str(syllable);
            if let Some(vowel) = vowel {
                output.push(vowel);
            }
            last_vowel = vowel
                .or_else(|| syllable.chars().last())
                .filter(|ch| "aeiou".contains(*ch));
        } else {
            if geminate {
                output.push_str("tsu");
            }
            output.push(raw);
            last_vowel = None;
        }
        geminate = false;
    }
    if geminate {
        output.push_str("tsu");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_gemination_yoon_long_vowels_and_syllabic_n() {
        assert_eq!(
            kana_to_romaji("がっこう、マッチ、しゃしん"),
            "gakkou、matchi、shashin"
        );
        assert_eq!(
            kana_to_romaji("スーパー ティー しんぶん しんよう"),
            "suupaa tii shimbun shin'you"
        );
        assert_eq!(
            kana_to_romaji("きゃりー ちゃん んあ 漢字"),
            "kyarii chan n'a 漢字"
        );
    }
}
