// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn columns_keep_their_one_line_widths_when_they_fit() {
    assert_eq!(
        column_widths(&[30., 60., 20.], &[40., 120., 30.], 500.),
        [40., 120., 30.]
    );
}

#[test]
fn short_columns_keep_their_words_when_long_ones_wrap() {
    // "Topic | Decision | Alternatives": a short column next to two long ones.
    let min = [60., 100., 100.];
    let max = [70., 900., 500.];
    let widths = column_widths(&min, &max, 600.);
    // The room above the narrowest widths (340) goes by how much more each wants.
    assert!((widths[0] - (60. + 10. * 340. / 1210.)).abs() < 0.01);
    assert!((widths.iter().sum::<f32>() - 600.).abs() < 0.01);
    assert!(widths[1] > widths[2]);
}

#[test]
fn words_break_only_when_even_they_do_not_fit() {
    let widths = column_widths(&[100., 300.], &[200., 600.], 200.);
    assert_eq!(widths, [50., 150.]);
    // A narrow column doesn't drop below the minimum, nor grows past its own width.
    assert_eq!(
        column_widths(&[30., 1000.], &[30., 1000.], 100.)[0],
        MIN_COLUMN
    );
    assert_eq!(column_widths(&[10., 1000.], &[10., 1000.], 100.)[0], 10.);
}
