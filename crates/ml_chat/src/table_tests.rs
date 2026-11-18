// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn columns_keep_their_natural_widths_when_they_fit() {
    assert_eq!(column_widths(&[40., 120., 30.], 500.), [40., 120., 30.]);
}

#[test]
fn columns_narrow_in_proportion_when_the_table_is_too_wide() {
    let widths = column_widths(&[100., 300.], 200.);
    assert_eq!(widths, [50., 150.]);
    // A narrow column doesn't drop below the minimum.
    let widths = column_widths(&[30., 1000.], 100.);
    assert_eq!(widths[0], MIN_COLUMN);
    // Nor grows past its natural width.
    let widths = column_widths(&[10., 1000.], 100.);
    assert_eq!(widths[0], 10.);
}
