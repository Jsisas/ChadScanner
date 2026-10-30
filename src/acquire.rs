//! Can the player buy this item instead of keeping a found one? Flea, trader cash offers and
//! barters, judged against the player's level, trader loyalty and finished tasks.
use crate::data::{Catalog, Item};
use crate::progress::Progress;

#[derive(Debug, Clone, PartialEq)]
pub enum Buy {
    Flea(i64),
    Trader { trader: String, level: u32, price: i64 },
    /// Barter whose inputs can all be bought now; `cost` is their summed price.
    Barter { trader: String, level: u32, cost: i64 },
}

impl Buy {
    pub fn cost(&self) -> i64 {
        match self {
            Buy::Flea(p) => *p,
            Buy::Trader { price, .. } => *price,
            Buy::Barter { cost, .. } => *cost,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Availability {
    /// Cheapest way to get it right now.
    Now(Buy),
    /// Not obtainable yet; the flea market (which lists it) opens at this player level.
    FleaAt(u32),
    /// No flea listing and no reachable trader offer or barter: only found copies count.
    Collect,
    /// Player level unknown, so flea and loyalty cannot be judged.
    Unknown,
}

fn task_done(p: &Progress, id: &Option<String>) -> bool {
    id.as_ref().is_none_or(|t| p.tasks.get(t).is_some_and(|e| e.complete && !e.failed && !e.invalid))
}

/// Reported loyalty decides; otherwise the player level must reach that loyalty level's requirement
/// (reputation and spent money are not known, so this is optimistic above LL1).
fn loyalty_reached(cat: &Catalog, p: &Progress, trader: &str, ll: u32) -> Option<bool> {
    if ll <= 1 {
        return Some(true);
    }
    if let Some(l) = p.traders.get(trader).and_then(|s| s.level) {
        return Some(l >= ll);
    }
    let need = cat.trader_levels.get(trader)?.iter().find(|(l, _)| *l == ll).map_or(0, |(_, r)| *r);
    p.level.map(|lvl| u32::from(lvl) >= need)
}

/// Cheapest cash way to get one now (flea or trader), ignoring barters.
fn cash(cat: &Catalog, item: &Item, p: &Progress) -> Option<Buy> {
    let flea = (!item.no_flea && p.level.is_some_and(|l| u32::from(l) >= cat.flea_min_level))
        .then_some(item.avg24h)
        .flatten()
        .map(Buy::Flea);
    let trader = item
        .buy_from_trader
        .iter()
        .filter(|o| task_done(p, &o.task_unlock) && loyalty_reached(cat, p, &o.trader, o.min_trader_level) == Some(true))
        .min_by_key(|o| o.price_rub)
        .map(|o| Buy::Trader { trader: o.trader.clone(), level: o.min_trader_level, price: o.price_rub });
    flea.into_iter().chain(trader).min_by_key(Buy::cost)
}

pub fn availability(cat: &Catalog, item: &Item, p: &Progress) -> Availability {
    let barter = cat
        .barters
        .iter()
        .filter(|b| b.offered.as_deref() == Some(item.id.as_str()))
        .filter(|b| task_done(p, &b.task_unlock) && loyalty_reached(cat, p, &b.trader, b.min_trader_level) == Some(true))
        .filter_map(|b| {
            // A barter counts only if every input can itself be bought now.
            let cost = b
                .inputs
                .iter()
                .map(|(id, n)| Some((cash(cat, cat.items.get(id)?, p)?.cost() as f64 * n) as i64))
                .sum::<Option<i64>>()?;
            Some(Buy::Barter { trader: b.trader.clone(), level: b.min_trader_level, cost })
        })
        .min_by_key(Buy::cost);
    if let Some(best) = cash(cat, item, p).into_iter().chain(barter).min_by_key(Buy::cost) {
        return Availability::Now(best);
    }
    match p.level {
        None => Availability::Unknown,
        Some(l) if !item.no_flea && item.avg24h.is_some() && u32::from(l) < cat.flea_min_level => {
            Availability::FleaAt(cat.flea_min_level)
        }
        Some(_) => Availability::Collect,
    }
}

/// Whether a non-FIR need for this item can be bought later rather than kept: obtainable now for at
/// most `max_price` each. FIR needs never qualify — bought items are not found in raid.
pub fn buy_later(a: &Availability, max_price: i64) -> bool {
    matches!(a, Availability::Now(b) if max_price > 0 && b.cost() <= max_price)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{Recipe, TraderOffer};
    use crate::progress::TaskProgress;

    fn cat() -> Catalog {
        let mut c = Catalog { flea_min_level: 15, ..Default::default() };
        c.trader_levels.insert("prapor".into(), vec![(1, 0), (2, 15), (3, 26)]);
        c
    }

    fn item(id: &str, avg: Option<i64>, no_flea: bool) -> Item {
        Item { id: id.into(), short_name: id.into(), width: 1, height: 1, avg24h: avg, no_flea, ..Default::default() }
    }

    fn at_level(l: u8) -> Progress {
        Progress { level: Some(l), ..Default::default() }
    }

    #[test]
    fn flea_locked_below_level_means_collect_until_then() {
        let c = cat();
        let shus = item("shus", Some(50_000), false);
        assert_eq!(availability(&c, &shus, &at_level(10)), Availability::FleaAt(15));
        assert_eq!(availability(&c, &shus, &at_level(16)), Availability::Now(Buy::Flea(50_000)));
        assert_eq!(availability(&c, &shus, &Progress::default()), Availability::Unknown);
    }

    #[test]
    fn trader_offer_needs_loyalty_and_unlock_task() {
        let mut c = cat();
        let mut bolts = item("bolts", None, true);
        bolts.buy_from_trader =
            vec![TraderOffer { trader: "prapor".into(), price_rub: 9_000, min_trader_level: 2, task_unlock: Some("t".into()) }];
        c.items.insert("bolts".into(), bolts.clone());
        assert_eq!(availability(&c, &bolts, &at_level(20)), Availability::Collect);
        let mut p = at_level(20);
        p.tasks.insert("t".into(), TaskProgress { complete: true, ..Default::default() });
        assert_eq!(availability(&c, &bolts, &p), Availability::Now(Buy::Trader { trader: "prapor".into(), level: 2, price: 9_000 }));
        assert_eq!(availability(&c, &bolts, &{ let mut q = p.clone(); q.level = Some(10); q }), Availability::Collect);
    }

    #[test]
    fn barter_counts_only_when_inputs_are_buyable() {
        let mut c = cat();
        let gpu = item("gpu", None, true);
        c.items.insert("gpu".into(), gpu.clone());
        c.items.insert("cheap".into(), item("cheap", Some(1_000), false));
        c.items.insert("rare".into(), item("rare", None, true));
        let barter = |input: &str| Recipe {
            inputs: vec![(input.into(), 3.0)],
            offered: Some("gpu".into()),
            trader: "prapor".into(),
            min_trader_level: 1,
            task_unlock: None,
        };
        c.barters = vec![barter("rare")];
        assert_eq!(availability(&c, &gpu, &at_level(20)), Availability::Collect);
        c.barters.push(barter("cheap"));
        assert_eq!(availability(&c, &gpu, &at_level(20)), Availability::Now(Buy::Barter { trader: "prapor".into(), level: 1, cost: 3_000 }));
    }
}
