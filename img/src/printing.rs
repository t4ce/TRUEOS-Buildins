//! The full decoded image, aspect-fitted and centered on a white A4 page.

use trueos::print2d::{self, a4};

pub(super) fn render_page(image: &crate::Image) -> Result<a4::Page, print2d::Error> {
    let mut page = a4::Page::white()?;
    page.fit_image_centered(&image.rgba, image.width, image.height, 10)?;
    Ok(page)
}
