//! The stock catalogue of `luma/film_look.py`: `catalog`, `entry`, `known`, `short_name`, `by_maker`, `find`,
//! `grain_settings`, `with_film`, `active`, `KINDS`, `MAKERS`.

use std::collections::HashMap;
use std::sync::OnceLock;

use grainy_core::Settings;
use serde_json::{Map, Value};

use crate::embedded;

/// The stock groups and their (Korean) titles, in Presets panel order (`film_look.KINDS`).
pub const KINDS: [(&str, &str); 4] = [
    ("colour-negative", "컬러 네거티브"),
    ("colour-slide", "컬러 슬라이드"),
    ("bw-negative", "흑백"),
    ("cine", "영화용 필름"),
];
/// Makers listed first in the Presets panel (`film_look.MAKERS`).
pub const MAKERS: [&str; 3] = ["Kodak", "Fujifilm", "Ilford"];
/// A maker with more films than this, of several kinds, gets a folder per kind (`film_look.SPLIT`).
pub const SPLIT: usize = 10;

/// One catalogue entry (an object of `assets/film/index.json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The stock id.
    pub id: String,
    /// The display name.
    pub name: String,
    /// The maker ('' when none).
    pub maker: String,
    /// 'colour-negative', 'colour-slide' or 'bw-negative'.
    pub kind: String,
    /// The Presets panel group (a kind, or 'cine').
    pub group: String,
    /// The entry as stored.
    pub raw: Value,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_owned()
}

/// Port of `film_look.catalog`: the shipped stocks in display order.
pub fn catalog() -> &'static [Entry] {
    static DATA: OnceLock<Vec<Entry>> = OnceLock::new();
    DATA.get_or_init(|| {
        let list: Value = serde_json::from_str(embedded::INDEX).unwrap_or(Value::Array(Vec::new()));
        list.as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|e| Entry {
                        id: text(&e["id"]),
                        name: text(&e["name"]),
                        maker: text(&e["maker"]),
                        kind: text(&e["kind"]),
                        group: text(&e["group"]),
                        raw: e.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// Port of `film_look.entry`: the catalogue entry of stock `name`, or `None`.
pub fn entry(name: &str) -> Option<&'static Entry> {
    if name.is_empty() {
        return None;
    }
    catalog().iter().find(|e| e.id == name)
}

/// Port of `film_look.known`.
pub fn known(name: &str) -> bool {
    entry(name).is_some()
}

/// Port of `film_look.short_name`: a stock's name inside its maker's folder ('Portra 400' for
/// 'Kodak Portra 400').
pub fn short_name(maker: &str, name: &str) -> String {
    if !maker.is_empty() {
        if let Some(rest) = name.strip_prefix(maker).and_then(|r| r.strip_prefix(' ')) {
            return rest.to_owned();
        }
    }
    name.to_owned()
}

/// A maker's folder in the Presets panel: kind title (`None`: one plain list) and its entries.
pub type Folder = (Option<&'static str>, Vec<&'static Entry>);

/// Port of `film_look.by_maker`: `[(maker, [(kind title or None, [entries])])]` in Presets panel order.
pub fn by_maker() -> Vec<(String, Vec<Folder>)> {
    let mut makers: Vec<(String, Vec<&'static Entry>)> = Vec::new();
    for e in catalog() {
        match makers.iter_mut().find(|(m, _)| *m == e.maker) {
            Some((_, list)) => list.push(e),
            None => makers.push((e.maker.clone(), vec![e])),
        }
    }
    makers.sort_by_key(|(m, _)| (MAKERS.iter().position(|x| x == m).unwrap_or(MAKERS.len()), m.to_lowercase()));
    makers
        .into_iter()
        .map(|(maker, entries)| {
            let kinds: Vec<Folder> = KINDS
                .iter()
                .map(|(kind, title)| (Some(*title), entries.iter().copied().filter(|e| e.group == *kind).collect()))
                .filter(|(_, group): &Folder| !group.is_empty())
                .collect();
            let folders = if entries.len() > SPLIT && kinds.len() > 1 { kinds } else { vec![(None, entries)] };
            (maker, folders)
        })
        .collect()
}

fn key(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The answer of [`find`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// No film ('', 'none', 'off', 'no film', `None`): the oracle's `('', None)`.
    Off,
    /// The stock id.
    Stock(String),
    /// Not found: up to five close names (possibly none).
    Suggestions(Vec<String>),
}

/// Port of `film_look.find`: a film named as people write it ('Kodak Portra 400', 'portra400', an id).
/// `None` stands for Python's `None` (read as 'None', which means no film).
pub fn find(text: Option<&str>) -> Found {
    let wanted = key(text.unwrap_or("None"));
    if wanted.is_empty() || ["none", "off", "nofilm"].contains(&wanted.as_str()) {
        return Found::Off;
    }
    let entries = catalog();
    if let Some(e) = entries.iter().find(|e| wanted == key(&e.id) || wanted == key(&e.name)) {
        return Found::Stock(e.id.clone());
    }
    let ending: Vec<&Entry> = entries.iter().filter(|e| key(&e.name).ends_with(&wanted)).collect();
    if ending.len() == 1 {
        return Found::Stock(ending[0].id.clone());
    }
    let partial: Vec<&Entry> = entries.iter().filter(|e| key(&e.name).contains(&wanted)).collect();
    if partial.len() == 1 {
        return Found::Stock(partial[0].id.clone());
    }
    if !partial.is_empty() {
        return Found::Suggestions(partial.iter().take(5).map(|e| e.name.clone()).collect());
    }
    // {key(name): name}: a later entry with the same key replaces the name, keeping the first position.
    let mut names: Vec<(String, String)> = Vec::new();
    for e in entries {
        let k = key(&e.name);
        match names.iter_mut().find(|(n, _)| *n == k) {
            Some(slot) => slot.1 = e.name.clone(),
            None => names.push((k, e.name.clone())),
        }
    }
    let keys: Vec<&str> = names.iter().map(|(k, _)| k.as_str()).collect();
    let lookup: HashMap<&str, &str> = names.iter().map(|(k, n)| (k.as_str(), n.as_str())).collect();
    Found::Suggestions(
        difflib::get_close_matches(&wanted, &keys, 5, 0.4).into_iter().map(|k| lookup[k].to_owned()).collect(),
    )
}

/// Port of `film_look.grain_settings`: the Effects › Grain values of a stock, or `None`.
pub fn grain_settings(name: &str) -> Option<Map<String, Value>> {
    let e = entry(name)?;
    match e.raw.get("grain") {
        Some(Value::Object(m)) if !m.is_empty() => Some(m.clone()),
        _ => None,
    }
}

fn python_float(value: Option<&Value>, missing: f64) -> f64 {
    match value {
        None => missing,
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    }
}

/// Port of `film_look.with_film`: `settings` with stock `name` ('' = none) as choosing it in the Film panel
/// makes them: the stock's grain goes into Effects › Grain, and a previous stock's grain leaves with it
/// unless it was changed. Keys in `explicit` are left as they are.
pub fn with_film(settings: &Settings, name: &str, explicit: &[&str]) -> Settings {
    let mut result = settings.clone();
    let previous = result.0.get("film_look").and_then(Value::as_str).unwrap_or("").to_owned();
    if let Some(before) = grain_settings(&previous) {
        let unchanged = before
            .iter()
            .all(|(k, v)| (python_float(result.0.get(k), 0.0) - v.as_f64().unwrap_or(f64::NAN)).abs() < 1e-6);
        if unchanged {
            let defaults = grainy_core::defaults();
            for k in before.keys() {
                if !explicit.contains(&k.as_str()) {
                    result.0.insert(k.clone(), defaults.value(k).clone());
                }
            }
        }
    }
    for (k, v) in grain_settings(name).unwrap_or_default() {
        if !explicit.contains(&k.as_str()) {
            result.0.insert(k, v);
        }
    }
    result.0.insert("film_look".into(), Value::String(name.to_owned()));
    result
}

/// Port of `film_look.active`: whether a develop with these settings uses a film look (HDR editing has
/// none).
pub fn active(settings: &Settings) -> bool {
    let name = settings.0.get("film_look");
    let named = name.is_some_and(grainy_core::settings::truthy);
    let hdr = settings.0.get("hdr").is_some_and(grainy_core::settings::truthy);
    named
        && !hdr
        && python_float(settings.0.get("film_strength"), 100.0) > 0.0
        && name.and_then(Value::as_str).is_some_and(known)
}

/// Port of the parts of Python's `difflib` that `get_close_matches` uses (no junk: the words are short).
mod difflib {
    use std::collections::HashMap;

    struct Matcher<'a> {
        a: Vec<char>,
        b: &'a [char],
        b2j: &'a HashMap<char, Vec<usize>>,
    }

    impl Matcher<'_> {
        fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
            let (a, b) = (&self.a, self.b);
            let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
            let mut j2len: HashMap<usize, usize> = HashMap::new();
            for i in alo..ahi {
                let mut next: HashMap<usize, usize> = HashMap::new();
                if let Some(js) = self.b2j.get(&a[i]) {
                    for &j in js {
                        if j < blo {
                            continue;
                        }
                        if j >= bhi {
                            break;
                        }
                        let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                        next.insert(j, k);
                        if k > bestsize {
                            besti = i + 1 - k;
                            bestj = j + 1 - k;
                            bestsize = k;
                        }
                    }
                }
                j2len = next;
            }
            while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
                besti -= 1;
                bestj -= 1;
                bestsize += 1;
            }
            while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
                bestsize += 1;
            }
            (besti, bestj, bestsize)
        }

        fn matches(&self) -> usize {
            let mut queue = vec![(0, self.a.len(), 0, self.b.len())];
            let mut total = 0;
            while let Some((alo, ahi, blo, bhi)) = queue.pop() {
                let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
                if k > 0 {
                    total += k;
                    if alo < i && blo < j {
                        queue.push((alo, i, blo, j));
                    }
                    if i + k < ahi && j + k < bhi {
                        queue.push((i + k, ahi, j + k, bhi));
                    }
                }
            }
            total
        }

        fn ratio(&self) -> f64 {
            let length = self.a.len() + self.b.len();
            if length == 0 {
                1.0
            } else {
                2.0 * self.matches() as f64 / length as f64
            }
        }
    }

    /// Port of `difflib.get_close_matches(word, possibilities, n, cutoff)`.
    pub(super) fn get_close_matches<'p>(word: &str, possibilities: &[&'p str], n: usize, cutoff: f64) -> Vec<&'p str> {
        let b: Vec<char> = word.chars().collect();
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (j, c) in b.iter().enumerate() {
            b2j.entry(*c).or_default().push(j);
        }
        let mut scored: Vec<(f64, &str)> = Vec::new();
        for &x in possibilities {
            let m = Matcher { a: x.chars().collect(), b: &b, b2j: &b2j };
            // real_quick_ratio and quick_ratio are upper bounds of ratio: only ratio decides.
            let r = m.ratio();
            if r >= cutoff {
                scored.push((r, x));
            }
        }
        scored.sort_by(|p, q| q.0.total_cmp(&p.0).then_with(|| q.1.cmp(p.1)));
        scored.into_iter().take(n).map(|(_, x)| x).collect()
    }
}
