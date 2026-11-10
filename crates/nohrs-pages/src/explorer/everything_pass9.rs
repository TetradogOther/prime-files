//! Passada 9 (docs/PERFORMANCE.md): the search as it was before it, kept
//! literally so the benchmark and the equivalence tests run the old code and
//! not a rewrite of it (OTIMIZACAO.md §1.3, "benchmark que reimplementa").
//! Only the names changed (`old_*`), and the calls between them.

#![allow(clippy::all, dead_code)]

use super::*;

fn old_search(
    this: &Segment,
    query: &Query,
    fuzzy: bool,
    cancel: &AtomicBool,
) -> Vec<(u32, u16, u32)> {
    // A drive none of the `em:` folders is on is not scanned at all.
    if this.paths.is_empty() && !query.scopes.is_empty() {
        let root = this.root().to_lowercase();
        if !query.scopes.iter().any(|scope| scope.starts_with(&root)) {
            return Vec::new();
        }
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, 16);
    let chunk = this.entries.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = this
            .entries
            .chunks(chunk)
            .enumerate()
            .map(|(part, entries)| {
                scope.spawn(move || {
                    let mut found = Vec::new();
                    for (offset, entry) in entries.iter().enumerate() {
                        if offset % 65_536 == 0 && cancel.load(Ordering::Relaxed) {
                            return found;
                        }
                        let index = (part * chunk + offset) as u32;
                        if let Some(rank) = old_rank(query, this, index, entry, fuzzy) {
                            found.push((rank, entry.name_len, index));
                        }
                    }
                    found
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_default())
            .collect()
    })
}

fn old_rank(
    this: &Query,
    segment: &Segment,
    position: u32,
    entry: &Entry,
    fuzzy: bool,
) -> Option<u32> {
    if entry.mask & this.mask != this.mask {
        return None;
    }
    let dir = entry.flags & DIR != 0;
    if this.kind.is_some_and(|want| want != dir) {
        return None;
    }
    if let Some((low, high)) = this.size
        && (dir || entry.size == UNKNOWN_SIZE || entry.size < low || entry.size > high)
    {
        return None;
    }
    if let Some((from, to)) = this.modified
        && (entry.modified == 0 || entry.modified < from || entry.modified > to)
    {
        return None;
    }
    let name = segment.lower(entry);
    if !this.exts.is_empty() || !this.excluded_exts.is_empty() {
        let ext = if dir {
            None
        } else {
            name.rsplit_once('.').map(|(_, ext)| ext)
        };
        if !this.exts.is_empty() && !ext.is_some_and(|ext| this.exts.iter().any(|want| want == ext))
        {
            return None;
        }
        if ext.is_some_and(|ext| this.excluded_exts.iter().any(|unwanted| unwanted == ext)) {
            return None;
        }
    }
    // Name terms first: building a path walks the parents, so it is done
    // only for entries every name term already let through.
    let names_match = this.terms.iter().all(|term| match term {
        Term::Name(part) if fuzzy => !name.contains(part.as_str()) && old_subsequence(part, name),
        Term::Name(part) => name.contains(part.as_str()),
        Term::Pattern(pattern) => wildcard(pattern, name),
        Term::Path(_) => true,
    });
    if !names_match {
        return None;
    }
    let excluded_by_name = this.excluded.iter().any(|term| match term {
        Term::Name(part) => name.contains(part.as_str()),
        Term::Pattern(pattern) => wildcard(pattern, name),
        Term::Path(_) => false,
    });
    if excluded_by_name {
        return None;
    }
    if this.needs_path {
        let path = segment.path(position).to_lowercase();
        if !this.scopes.is_empty()
            && !this
                .scopes
                .iter()
                .any(|scope| path.starts_with(scope.as_str()))
        {
            return None;
        }
        let paths_match = this.terms.iter().all(|term| match term {
            Term::Path(part) => path.contains(part.as_str()),
            _ => true,
        });
        let excluded_by_path = this.excluded.iter().any(|term| match term {
            Term::Path(part) => path.contains(part.as_str()),
            _ => false,
        });
        if !paths_match || excluded_by_path {
            return None;
        }
    }
    let class = if fuzzy {
        5
    } else if this.whole.is_empty() || name == this.whole {
        0
    } else if name.starts_with(&this.whole) {
        1
    } else if this
        .first
        .as_deref()
        .is_some_and(|first| starts_a_word(name, first))
    {
        2
    } else {
        3
    };
    let noise = u32::from(entry.flags & NOISE != 0);
    Some(class << 8 | noise << 4 | recency(entry.modified, this.now))
}

fn old_subsequence(term: &str, name: &str) -> bool {
    let wanted: Vec<char> = term.chars().collect();
    let Some(&first) = wanted.first() else {
        return false;
    };
    let letters: Vec<char> = name.chars().collect();
    let reach = wanted.len() * 2;
    letters
        .iter()
        .enumerate()
        .filter(|(_, letter)| **letter == first)
        .any(|(start, _)| {
            let mut next = 1;
            for (at, letter) in letters.iter().enumerate().skip(start + 1) {
                if next == wanted.len() || at - start >= reach {
                    break;
                }
                if wanted.get(next) == Some(letter) {
                    next += 1;
                }
            }
            next == wanted.len()
        })
}

pub(super) fn old_gather(
    index: Option<&Index>,
    walking: Option<&Mutex<Segment>>,
    fresh: Option<&Mutex<Fresh>>,
    query: &Query,
    cancel: &AtomicBool,
) -> Vec<(String, String, bool)> {
    let table = walking.map(lock);
    let fresh = fresh.map(lock);
    let mut sources: Vec<(usize, &Segment)> = Vec::new();
    if let Some(index) = index {
        sources.extend(
            index
                .segments
                .iter()
                .enumerate()
                .map(|(at, segment)| (at, &**segment)),
        );
    }
    if let Some(table) = &table {
        sources.push((WALKING, &**table));
    }
    if let Some(fresh) = &fresh {
        sources.push((FRESH, &fresh.table));
    }
    let scan = |fuzzy: bool| {
        let mut hits: Vec<(u32, u16, usize, u32)> = Vec::new();
        for (source, segment) in &sources {
            hits.extend(
                segment
                    .search(query, fuzzy, cancel)
                    .into_iter()
                    .map(|(rank, length, position)| (rank, length, *source, position)),
            );
        }
        hits
    };
    let mut hits = scan(false);
    if hits.len() < FUZZY_BELOW && query.fuzzy.is_some() {
        let mut near = scan(true);
        near.sort_unstable();
        near.truncate(FUZZY_MOST);
        hits.extend(near);
    }
    hits.sort_unstable();
    // A file the watcher saw may also be in a table walked since.
    let mut shown = std::collections::HashSet::new();
    hits.into_iter()
        .filter_map(|(_, _, source, position)| {
            let segment = sources.iter().find(|(id, _)| *id == source)?.1;
            let entry = segment.entries.get(position as usize)?;
            let path = segment.path(position);
            shown.insert(path.to_lowercase()).then(|| {
                (
                    segment.name(entry).to_string(),
                    path,
                    entry.flags & DIR != 0,
                )
            })
        })
        .take(MAX_RESULTS)
        .collect()
}

// ---------------------------------------------------------------------------
// The table both sides search

const WORDS: [&str; 24] = [
    "render", "final", "bonk", "Projeto", "audio", "cena", "take", "export", "IMG", "backup", "v2",
    "kick", "Música", "ação", "whoosh", "impact", "Wood", "hits", "voz", "trilha", "cut", "glow",
    "shake", "ΣΑΣ",
];
const EXTS: [&str; 8] = ["wav", "mp3", "png", "prproj", "mp4", "txt", "WAV", "dat"];

/// A drive of `tops` × `subs` folders holding `files` names each, in the
/// order a walk adds them (a folder's children together), with sizes and
/// dates spread so every filter has something to keep and to drop.
pub(super) fn synthetic(tops: u32, subs: u32, files: u32) -> Index {
    let mut table = Segment::empty();
    let drive = table.add(
        NO_PARENT,
        "C:",
        Found {
            dir: true,
            size: 0,
            modified: 1,
        },
    );
    let mut seed = 7u64;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed >> 33
    };
    let now = clamp_seconds(1_793_880_000);
    for top in 0..tops {
        let name = if top % 7 == 0 {
            format!("Program Files {top:02}")
        } else {
            format!("Pasta{top:02} {}", WORDS[top as usize % WORDS.len()])
        };
        let mut top_id = table.add(
            drive,
            &name,
            Found {
                dir: true,
                size: 0,
                modified: now,
            },
        );
        // The depth of a real drive's files (C:\Users\x\Documents\...): a
        // path filter climbs it for every name it looks at.
        for level in ["Users", "editor", "Documents", "Projetos 2026"] {
            top_id = table.add(
                top_id,
                level,
                Found {
                    dir: true,
                    size: 0,
                    modified: now,
                },
            );
        }
        for sub in 0..subs {
            let word = WORDS[next() as usize % WORDS.len()];
            let sub_id = table.add(
                top_id,
                &format!("sub{sub:02} {word}"),
                Found {
                    dir: true,
                    size: 0,
                    modified: now,
                },
            );
            for file in 0..files {
                let first = WORDS[next() as usize % WORDS.len()];
                let second = WORDS[next() as usize % WORDS.len()];
                let ext = EXTS[next() as usize % EXTS.len()];
                let age = (next() % 400) as u32 * 86_400;
                table.add(
                    sub_id,
                    &format!("{first}_{second} {file:04}.{ext}"),
                    Found {
                        dir: false,
                        size: next() % 50_000_000,
                        modified: now.saturating_sub(age),
                    },
                );
            }
        }
    }
    table.complete = true;
    Index {
        segments: vec![Arc::new(table)],
        built: SystemTime::now(),
    }
}

/// The queries both sides are held to: broad and narrow, every filter, the
/// path terms and scopes item 2 touches, and the near-miss pass item 3 does.
pub(super) const QUERIES: [&str; 18] = [
    "a",
    "re",
    "render",
    "bonk render",
    "kick 0001",
    "bnkrndr",
    "rendr",
    "ext:wav",
    "ext:wav -kick",
    r#"em:"C:\Pasta03 Projeto" wav"#,
    r#"em:"C:\Program Files 07" final"#,
    "em:sub07 final",
    r"2026\sub0",
    r"-sub1\ glow",
    "σας",
    r"σας\ kick",
    r#"em:"C:\Pasta03 Projeto" -2026\sub05 wav"#,
    "*.prproj tamanho:>10mb",
];

fn parse(text: &str) -> Query {
    Query::parse_at(
        text,
        r"C:\Pasta01 audio",
        time::OffsetDateTime::from_unix_timestamp(1_793_880_000).unwrap(),
    )
    .unwrap()
}

#[test]
fn the_search_finds_what_it_found_before_pass_9() {
    let index = synthetic(8, 8, 300);
    for text in QUERIES {
        let query = parse(text);
        let cancel = AtomicBool::new(false);
        let before = old_gather(Some(&index), None, None, &query, &cancel);
        let after = gather(Some(&index), None, None, &query, &cancel);
        assert!(
            !before.is_empty() || text == "σας",
            "{text:?} finds nothing, so it proves nothing"
        );
        assert_eq!(after, before, "{text:?}");
    }
}

// cargo test -p nohrs-pages --release pass_9_speed -- --ignored --nocapture
// NOHRS_PASS9_INDEX=<path of everything.idx> runs it on a saved table instead
// of the synthetic one (it needs the table's size in free memory).
#[test]
#[ignore]
fn pass_9_speed() {
    let index = match std::env::var("NOHRS_PASS9_INDEX") {
        Ok(path) => load(Path::new(&path)).expect("a readable everything.idx"),
        Err(_) => synthetic(50, 50, 1_000),
    };
    println!("{} names", index.len());
    for text in QUERIES {
        let query = parse(text);
        let cancel = AtomicBool::new(false);
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut hits = 0;
        // Alternated, so a slower moment of the machine falls on both sides.
        for _ in 0..5 {
            let started = Instant::now();
            let old = old_gather(Some(&index), None, None, &query, &cancel);
            before.push(started.elapsed());
            let started = Instant::now();
            let new = gather(Some(&index), None, None, &query, &cancel);
            after.push(started.elapsed());
            assert_eq!(
                new, old,
                "{text:?}: the two sides differ, nothing to compare"
            );
            hits = new.len();
        }
        before.sort();
        after.sort();
        println!(
            "{text:>32}: {hits:>5} hits, before {:>9.1?}, after {:>9.1?}",
            before[2], after[2]
        );
    }
}

// Item 1: the order handed out is the whole sort's, read as far as wanted.
#[test]
fn the_hits_come_in_the_order_of_sorting_them_all() {
    let mut seed = 3u64;
    for length in [0usize, 1, 2, 7, 100, 5_001, 20_000] {
        for first in [1usize, 3, 100, 5_000] {
            let mut items: Vec<(u32, u32)> = (0..length as u32)
                .map(|position| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((seed >> 40) as u32 % 50, position)
                })
                .collect();
            let mut sorted = items.clone();
            sorted.sort_unstable();
            // Stops at random places past the first block, as the dedup does.
            let stop = (seed as usize % (length + 1)).max(first.min(length));
            let mut read = Vec::new();
            in_order_while(&mut items, first, |item| {
                read.push(item);
                read.len() < stop
            });
            assert_eq!(
                read,
                sorted[..stop.min(length)],
                "{length} items, first {first}, stop at {stop}"
            );
        }
    }
}

thread_local! {
    static COMPARED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

// Every comparison goes through `cmp`: a derived `PartialOrd` would let the
// sort compare without counting, and a full sort would pass.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Counted(u64);

impl PartialOrd for Counted {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Counted {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        COMPARED.with(|count| count.set(count.get() + 1));
        self.0.cmp(&other.0)
    }
}

// Reading the first 5,000 of a million costs a pass over them, not a sort:
// a full sort is ~20 million comparisons, this ~3 million.
#[test]
fn reading_the_first_hits_does_not_sort_them_all() {
    let mut seed = 11u64;
    let mut items: Vec<Counted> = (0..1_000_000)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            Counted(seed >> 1)
        })
        .collect();
    COMPARED.with(|count| count.set(0));
    let mut read = 0;
    in_order_while(&mut items, MAX_RESULTS, |_| {
        read += 1;
        read < MAX_RESULTS
    });
    let compared = COMPARED.with(std::cell::Cell::get);
    assert_eq!(read, MAX_RESULTS);
    assert!(compared < 6_000_000, "{compared} comparisons");
}

// Item 3: the near-miss test answers as the old one did, for any pair.
#[test]
fn the_near_miss_test_answers_as_before() {
    let letters: Vec<char> = "abknorçãσς _.0".chars().collect();
    let mut seed = 5u64;
    let mut text = |most: u64| {
        let mut out = String::new();
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        for _ in 0..(seed >> 40) % (most + 1) {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            out.push(letters[(seed >> 33) as usize % letters.len()]);
        }
        out
    };
    let mut both = 0;
    for _ in 0..200_000 {
        let term = text(5);
        let name = text(24);
        let old = old_subsequence(&term, &name);
        assert_eq!(subsequence(&term, &name), old, "{term:?} in {name:?}");
        both += usize::from(old);
    }
    assert!(
        both > 1_000,
        "only {both} pairs match: the test would not see a wrong `true`"
    );
}

// Items 2 and 3 alone, on one thread, alternated in one run: the whole
// search mixes them with the scan and the sort, and this machine's noise
// (another program's memory) is larger than either.
// cargo test -p nohrs-pages --release pass_9_parts_speed -- --ignored --nocapture
#[test]
#[ignore]
fn pass_9_parts_speed() {
    let index = synthetic(50, 50, 1_000);
    let segment = &*index.segments[0];
    let count = segment.entries.len() as u32;
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        let started = Instant::now();
        let old: usize = (0..count)
            .map(|position| segment.path(position).to_lowercase().len())
            .sum();
        before.push(started.elapsed());
        let started = Instant::now();
        let mut paths = LowerPaths::default();
        let new: usize = (0..count)
            .map(|position| paths.of(segment, position).len())
            .sum();
        after.push(started.elapsed());
        assert_eq!(new, old);
    }
    before.sort();
    after.sort();
    println!(
        "lowered paths of {count} names: before {:.1?}, after {:.1?}",
        before[2], after[2]
    );

    let names: Vec<&str> = segment
        .entries
        .iter()
        .map(|entry| segment.lower(entry))
        .collect();
    let (mut before, mut after) = (Vec::new(), Vec::new());
    for _ in 0..5 {
        let started = Instant::now();
        let old = names
            .iter()
            .filter(|name| old_subsequence("rendr", name))
            .count();
        before.push(started.elapsed());
        let started = Instant::now();
        let new = names
            .iter()
            .filter(|name| subsequence("rendr", name))
            .count();
        after.push(started.elapsed());
        assert_eq!(new, old);
    }
    before.sort();
    after.sort();
    println!(
        "near-miss test over {count} names: before {:.1?}, after {:.1?}",
        before[2], after[2]
    );
}

// Item 2: a parent's path is built once for its children, not per child.
#[test]
fn a_folder_path_is_built_once_for_all_its_children() {
    let index = synthetic(3, 4, 200);
    let segment = &*index.segments[0];
    let mut paths = LowerPaths::default();
    for position in 0..segment.entries.len() as u32 {
        assert_eq!(
            paths.of(segment, position),
            segment.path(position).to_lowercase()
        );
    }
    let folders = segment
        .entries
        .iter()
        .filter(|entry| entry.flags & DIR != 0)
        .count();
    assert!(
        paths.built <= folders * 2,
        "{} parents built for {folders} folders",
        paths.built
    );
}
