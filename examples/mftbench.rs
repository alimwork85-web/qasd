//! Compare the MFT listing (`mft`) with the folder crawl (`walk`) per drive:
//! time, entry count, and paths only one of them has. Needs administrator
//! rights for the MFT side. Usage: mftbench <out.txt>

use qasd::walk::{Listing, NONE};
use std::collections::{HashMap, HashSet};
use std::io::Write;

fn paths(ls: &[Listing], root: &[u8]) -> HashSet<Vec<u8>> {
    let by_id: HashMap<u32, &Listing> = ls.iter().map(|l| (l.id, l)).collect();
    let mut out = HashSet::new();
    let mut stack = vec![(0u32, root.to_vec())];
    while let Some((id, p)) = stack.pop() {
        let Some(l) = by_id.get(&id) else { continue };
        for e in &l.ents {
            let name = &l.names[e.name_off as usize..][..e.name_len as usize];
            let c = qasd::live::join(&p, name);
            if e.child != NONE {
                stack.push((e.child, c.clone()));
            }
            out.insert(c);
        }
    }
    out
}

fn main() {
    let out_path = std::env::args().nth(1).unwrap_or_else(|| "mftbench.txt".into());
    let mut out = std::fs::File::create(&out_path).unwrap();
    writeln!(out, "elevated: {}", qasd::sys::elevated()).unwrap();
    for d in qasd::sys::drives() {
        let name = qasd::sys::display(&d);
        let t = std::time::Instant::now();
        let m = qasd::mft::scan_volume(&d);
        let mt = t.elapsed();
        let t = std::time::Instant::now();
        let w = qasd::walk::scan(&d, 8);
        let wt = t.elapsed();
        let Some(m) = m else {
            writeln!(out, "{name}: MFT unavailable; crawl {wt:.2?}").unwrap();
            continue;
        };
        let (pm, pw) = (paths(&m, &d), paths(&w, &d));
        writeln!(out, "{name}: MFT {} entries in {mt:.2?}; crawl {} entries in {wt:.2?}", pm.len(), pw.len()).unwrap();
        let only_m: Vec<_> = pm.difference(&pw).take(15).map(|p| qasd::sys::display(p)).collect();
        let only_w: Vec<_> = pw.difference(&pm).take(15).map(|p| qasd::sys::display(p)).collect();
        writeln!(out, "  only in MFT: {}", pm.difference(&pw).count()).unwrap();
        for p in only_m {
            writeln!(out, "    {p}").unwrap();
        }
        // Where missing branches start: a missing path whose parent is there.
        let tops: Vec<_> = pw
            .difference(&pm)
            .filter(|p| p.iter().rposition(|&b| b == b'/').is_some_and(|c| pm.contains(&p[..c]) || p[..c] == d[..]))
            .collect();
        writeln!(out, "  missing branch tops: {}", tops.len()).unwrap();
        for p in tops.iter().take(4) {
            writeln!(out, "    {}", qasd::sys::display(p)).unwrap();
            // The MFT's paths ending in the same last two names.
            let comps: Vec<&[u8]> = p.rsplit(|&b| b == b'/').take(2).collect();
            let tail = [comps[1], b"/", comps[0]].concat();
            for m in pm.iter().filter(|m| m.ends_with(&tail)).take(6) {
                writeln!(out, "      MFT has: {}", qasd::sys::display(m)).unwrap();
            }
        }
        writeln!(out, "  only in crawl: {}", pw.difference(&pm).count()).unwrap();
        for p in only_w {
            writeln!(out, "    {p}").unwrap();
        }
    }
    writeln!(out, "done").unwrap();
}
