use super::EditorView;
use crate::editor_model::Folds;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockTone {
    Title,
    Text,
    Faint,
    Code,
    Suggestion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockRow {
    pub(crate) text: String,
    pub(crate) tone: BlockTone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) id: usize,
    pub(crate) line: usize,
    pub(crate) rows: Vec<BlockRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    Text(usize),
    Block { block: usize, row: usize },
}

#[derive(Debug, Clone, Copy)]
struct Anchor {
    row: usize,
    block: usize,
    rows: usize,
}

/// Where the blocks fall among the text rows. Blocks below a folded line are not shown.
#[derive(Debug, Default)]
pub(crate) struct BlockRows {
    anchors: Vec<Anchor>,
}

impl BlockRows {
    pub(crate) fn new(blocks: &[Block], folds: &Folds) -> Self {
        let mut anchors: Vec<Anchor> = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| !block.rows.is_empty() && !folds.is_hidden(block.line))
            .map(|(index, block)| Anchor {
                row: folds.row_of(block.line),
                block: index,
                rows: block.rows.len(),
            })
            .collect();
        anchors.sort_by_key(|anchor| anchor.row);
        Self { anchors }
    }

    pub(crate) fn count(&self, text_rows: usize) -> usize {
        text_rows + self.anchors.iter().map(|anchor| anchor.rows).sum::<usize>()
    }

    pub(crate) fn visual_of(&self, row: usize) -> usize {
        let above: usize = self
            .anchors
            .iter()
            .take_while(|anchor| anchor.row < row)
            .map(|anchor| anchor.rows)
            .sum();
        row + above
    }

    pub(crate) fn slot(&self, visual: usize) -> Slot {
        let mut above = 0;
        for anchor in &self.anchors {
            let start = anchor.row + above + 1;
            if visual < start {
                break;
            }
            if visual < start + anchor.rows {
                return Slot::Block {
                    block: anchor.block,
                    row: visual - start,
                };
            }
            above += anchor.rows;
        }
        Slot::Text(visual - above)
    }

    pub(crate) fn text_row(&self, visual: usize) -> usize {
        match self.slot(visual) {
            Slot::Text(row) => row,
            Slot::Block { block, .. } => self
                .anchors
                .iter()
                .find(|anchor| anchor.block == block)
                .map_or(0, |anchor| anchor.row),
        }
    }
}

impl EditorView {
    pub(crate) fn set_blocks(&mut self, blocks: Vec<Block>, cx: &mut gpui::Context<Self>) {
        if self.blocks != blocks {
            self.blocks = blocks;
            cx.notify();
        }
    }

    pub(super) fn block_rows(&self) -> BlockRows {
        BlockRows::new(&self.blocks, &self.model.folds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_model::{FoldKind, FoldRegion};

    fn block(line: usize, rows: usize) -> Block {
        Block {
            id: line,
            line,
            rows: vec![
                BlockRow {
                    text: String::new(),
                    tone: BlockTone::Text,
                };
                rows
            ],
        }
    }

    fn slots(rows: &BlockRows, count: usize) -> String {
        (0..count)
            .map(|visual| match rows.slot(visual) {
                Slot::Text(row) => row.to_string(),
                Slot::Block { block, row } => format!("b{block}.{row}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn without_blocks_rows_are_text_rows() {
        let rows = BlockRows::default();
        assert_eq!(rows.count(5), 5);
        assert_eq!(rows.visual_of(3), 3);
        assert_eq!(rows.slot(3), Slot::Text(3));
        assert_eq!(rows.text_row(4), 4);
    }

    #[test]
    fn blocks_sit_below_their_lines() {
        let blocks = [block(1, 2), block(3, 1), block(1, 1)];
        let rows = BlockRows::new(&blocks, &Folds::default());
        assert_eq!(rows.count(5), 9);
        assert_eq!(slots(&rows, 9), "0 1 b0.0 b0.1 b2.0 2 3 b1.0 4");
        let visuals: Vec<_> = (0..5).map(|row| rows.visual_of(row)).collect();
        assert_eq!(visuals, [0, 1, 5, 6, 8]);
        assert_eq!(rows.text_row(3), 1);
        assert_eq!(rows.text_row(4), 1);
        assert_eq!(rows.text_row(7), 3);
    }

    #[test]
    fn a_block_on_the_last_line_and_an_empty_block() {
        let blocks = [block(2, 1), block(0, 0)];
        let rows = BlockRows::new(&blocks, &Folds::default());
        assert_eq!(rows.count(3), 4);
        assert_eq!(slots(&rows, 4), "0 1 2 b0.0");
    }

    #[test]
    fn blocks_inside_a_fold_are_hidden_and_others_follow_the_fold() {
        let mut folds = Folds::default();
        folds.set_regions(vec![FoldRegion {
            header: 1,
            start: 1,
            end: 3,
            kind: FoldKind::Code,
        }]);
        folds.toggle(1);
        let blocks = [block(2, 2), block(1, 1), block(4, 1)];
        let rows = BlockRows::new(&blocks, &folds);
        assert_eq!(folds.row_count(6), 4);
        assert_eq!(rows.count(4), 6);
        assert_eq!(slots(&rows, 6), "0 1 b1.0 2 b2.0 3");
    }
}
