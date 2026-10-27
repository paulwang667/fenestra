//! A look can name its material style; the stock looks stay solid.

use fenestra_core::{MaterialStyle, Mode};
use fenestra_looks::{all, product};

#[test]
fn look_material_style_sets_the_theme() {
    let look = product(Mode::Dark).with_material_style(MaterialStyle::LiquidGlass);
    assert_eq!(look.theme.material_style, MaterialStyle::LiquidGlass);
    for look in all(Mode::Dark).into_iter().chain(all(Mode::Light)) {
        assert_eq!(
            look.theme.material_style,
            MaterialStyle::Solid,
            "{}",
            look.name
        );
    }
}
