//! Calendar periods and chart buckets use civil dates across clock changes.

use super::{Listening, round1};
use chrono::{Datelike, Days, Months, NaiveDate, NaiveDateTime, Timelike, Weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodKind {
    Day,
    Week,
    Month,
    Year,
    All,
}
#[derive(Debug)]
pub struct Unit {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub hour: Option<u32>,
}
#[derive(Debug)]
pub struct Period {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub previous: Option<(NaiveDate, NaiveDate)>,
    pub current: Option<usize>,
    pub units: Vec<Unit>,
}
impl Listening {
    /// None means a boundary falls outside the representable calendar.
    pub fn period(
        &self,
        kind: PeriodKind,
        when: NaiveDateTime,
        week_start: Weekday,
    ) -> Option<Period> {
        use PeriodKind::*;
        let today = when.date();
        let month = today.with_day(1)?;
        let year = month.with_month(1)?;
        let month_end = |d: NaiveDate| d.checked_add_months(Months::new(1))?.pred_opt();
        let mut units = vec![];
        let (from, to, previous, current) = match kind {
            Day => {
                units.extend((0..24).map(|h| Unit {
                    from: today,
                    to: today,
                    hour: Some(h),
                }));
                (
                    today,
                    today,
                    Some((today.pred_opt()?, today.pred_opt()?)),
                    Some(when.hour() as usize),
                )
            }
            Week => {
                let back = today.weekday().days_since(week_start) as u64;
                let start = today.checked_sub_days(Days::new(back))?;
                (
                    start,
                    start.checked_add_days(Days::new(6))?,
                    Some((start.checked_sub_days(Days::new(7))?, start.pred_opt()?)),
                    Some(back as usize),
                )
            }
            Month => (
                month,
                month_end(month)?,
                Some((month.checked_sub_months(Months::new(1))?, month.pred_opt()?)),
                Some(today.day0() as usize),
            ),
            Year => (
                year,
                month_end(year.with_month(12)?)?,
                Some((year.checked_sub_months(Months::new(12))?, year.pred_opt()?)),
                Some(today.month0() as usize),
            ),
            All => (self.first_day().unwrap_or(today), today, None, None),
        };
        if matches!(kind, Week | Month) {
            for d in from.iter_days().take_while(|d| *d <= to) {
                units.push(Unit {
                    from: d,
                    to: d,
                    hour: None,
                });
            }
        } else if matches!(kind, Year | All) {
            let mut start = from.with_day(1)?;
            if kind == All
                && let Some(cap) = month.checked_sub_months(Months::new(35))
            {
                start = start.max(cap);
            }
            while start <= to {
                units.push(Unit {
                    from: start,
                    to: month_end(start)?,
                    hour: None,
                });
                start = start.checked_add_months(Months::new(1))?;
            }
        }
        Some(Period {
            from,
            to,
            previous,
            current: current.or_else(|| units.len().checked_sub(1)),
            units,
        })
    }
    pub fn series(&self, p: &Period) -> Vec<f64> {
        p.units
            .iter()
            .map(|u| match u.hour {
                Some(h) => round1(self.days().get(&u.from).map_or(0., |d| d.h[h as usize])),
                None => round1(self.days().range(u.from..=u.to).map(|(_, d)| d.s).sum()),
            })
            .collect()
    }
}
