use crate::Commit;

/// One row of the commit graph, for the commit at the same index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    /// The lane (column, from 0 on the left) the commit's node is in.
    pub lane: usize,
    /// Lanes entering at the top of the row that end at the node (lines from children).
    pub into: Vec<usize>,
    /// Lanes at the bottom of the row that start at the node (lines to parents).
    pub out: Vec<usize>,
    /// Lines that pass the node: (lane at the top of the row, lane at the bottom).
    pub through: Vec<(usize, usize)>,
}

fn take_free(lanes: &mut Vec<Option<String>>, id: &str) -> usize {
    let slot = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    });
    if let Some(entry) = lanes.get_mut(slot) {
        *entry = Some(id.to_owned());
    }
    slot
}

/// Lays out `commits` (newest first, each child before its parents, as `git log` gives
/// them) as a graph. Parents missing from `commits` keep their lane to the last row.
pub fn layout(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let into: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, waiting)| waiting.as_deref() == Some(commit.id.as_str()))
            .map(|(i, _)| i)
            .collect();
        let through: Vec<(usize, usize)> = lanes
            .iter()
            .enumerate()
            .filter(|(i, waiting)| waiting.is_some() && !into.contains(i))
            .map(|(i, _)| (i, i))
            .collect();
        for &i in &into {
            if let Some(entry) = lanes.get_mut(i) {
                *entry = None;
            }
        }
        let lane = into.first().copied().unwrap_or_else(|| {
            let slot = take_free(&mut lanes, &commit.id);
            if let Some(entry) = lanes.get_mut(slot) {
                *entry = None;
            }
            slot
        });

        let mut out = Vec::new();
        for (n, parent) in commit.parents.iter().enumerate() {
            let waiting = lanes
                .iter()
                .position(|entry| entry.as_deref() == Some(parent.as_str()));
            let target = match waiting {
                Some(existing) => existing,
                None if n == 0 => {
                    if let Some(entry) = lanes.get_mut(lane) {
                        *entry = Some(parent.clone());
                    }
                    lane
                }
                None => take_free(&mut lanes, parent),
            };
            out.push(target);
        }
        out.sort_unstable();
        out.dedup();

        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        rows.push(GraphRow {
            lane,
            into,
            out,
            through,
        });
    }
    rows
}

/// The number of lanes the widest row uses; 0 for no rows.
pub fn width(rows: &[GraphRow]) -> usize {
    rows.iter()
        .flat_map(|row| {
            let through = row.through.iter().flat_map(|&(top, bottom)| [top, bottom]);
            std::iter::once(row.lane)
                .chain(row.into.iter().copied())
                .chain(row.out.iter().copied())
                .chain(through)
        })
        .max()
        .map_or(0, |max| max + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: String::new(),
            email: String::new(),
            time: 0,
            subject: String::new(),
            refs: Vec::new(),
            body: String::new(),
        }
    }

    fn row(lane: usize, into: &[usize], out: &[usize], through: &[(usize, usize)]) -> GraphRow {
        GraphRow {
            lane,
            into: into.to_vec(),
            out: out.to_vec(),
            through: through.to_vec(),
        }
    }

    #[test]
    fn no_commits() {
        assert_eq!(layout(&[]), []);
        assert_eq!(width(&[]), 0);
    }

    #[test]
    fn linear_history_stays_in_lane_zero() {
        let rows = layout(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(
            rows,
            [
                row(0, &[], &[0], &[]),
                row(0, &[0], &[0], &[]),
                row(0, &[0], &[], &[]),
            ]
        );
        assert_eq!(width(&rows), 1);
    }

    #[test]
    fn branch_and_merge() {
        let rows = layout(&[
            commit("m", &["a", "b"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("r", &[]),
        ]);
        assert_eq!(
            rows,
            [
                row(0, &[], &[0, 1], &[]),
                row(0, &[0], &[0], &[(1, 1)]),
                row(1, &[1], &[0], &[(0, 0)]),
                row(0, &[0], &[], &[]),
            ]
        );
        assert_eq!(width(&rows), 2);
    }

    #[test]
    fn two_children_waiting_for_one_commit_both_enter_it() {
        let rows = layout(&[
            commit("x", &["r"]),
            commit("y", &["z"]),
            commit("z", &["r"]),
            commit("r", &[]),
        ]);
        assert_eq!(rows[0], row(0, &[], &[0], &[]));
        assert_eq!(rows[1], row(1, &[], &[1], &[(0, 0)]));
        assert_eq!(rows[2], row(1, &[1], &[0], &[(0, 0)]));
        assert_eq!(rows[3], row(0, &[0], &[], &[]));
    }

    #[test]
    fn a_commit_with_several_children_in_different_lanes() {
        let rows = layout(&[
            commit("x", &["p"]),
            commit("y", &["q"]),
            commit("p", &["q"]),
            commit("q", &[]),
        ]);
        assert_eq!(rows[2], row(0, &[0], &[1], &[(1, 1)]));
        assert_eq!(rows[3], row(1, &[1], &[], &[]));
    }

    #[test]
    fn two_independent_heads() {
        let rows = layout(&[
            commit("a2", &["a1"]),
            commit("b2", &["b1"]),
            commit("a1", &[]),
            commit("b1", &[]),
        ]);
        assert_eq!(
            rows,
            [
                row(0, &[], &[0], &[]),
                row(1, &[], &[1], &[(0, 0)]),
                row(0, &[0], &[], &[(1, 1)]),
                row(1, &[1], &[], &[]),
            ]
        );
        assert_eq!(width(&rows), 2);
    }

    #[test]
    fn a_missing_parent_keeps_its_lane() {
        let rows = layout(&[commit("b", &["gone"]), commit("a", &["a0"])]);
        assert_eq!(rows, [row(0, &[], &[0], &[]), row(1, &[], &[1], &[(0, 0)])]);
    }

    #[test]
    fn octopus_merge() {
        let rows = layout(&[
            commit("m", &["a", "b", "c"]),
            commit("a", &["r"]),
            commit("b", &["r"]),
            commit("c", &["r"]),
            commit("r", &[]),
        ]);
        assert_eq!(rows[0], row(0, &[], &[0, 1, 2], &[]));
        assert_eq!(rows[1], row(0, &[0], &[0], &[(1, 1), (2, 2)]));
        assert_eq!(rows[2], row(1, &[1], &[0], &[(0, 0), (2, 2)]));
        assert_eq!(rows[3], row(2, &[2], &[0], &[(0, 0)]));
        assert_eq!(rows[4], row(0, &[0], &[], &[]));
        assert_eq!(width(&rows), 3);
    }
}
