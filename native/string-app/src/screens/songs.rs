use super::{
    detail, home,
    library::{self, count, duration},
};
use crate::{
    ui::{self, songs::Columns},
    view::View,
};
use egui::{Rect, Ui, Vec2, pos2};
pub fn show(ui: &mut Ui, v: &mut View<'_>) {
    if home::empty(ui, v) {
        return;
    }
    let all = v.s.models.songs.clone();
    let ctx = v.context();
    let field_width = if ui.available_width() < 690. {
        148.
    } else {
        220.
    };
    let r = library::title_bar(
        ui,
        66.,
        "Songs",
        &format!(
            "{} · {}",
            count(all.len(), "song"),
            duration(v.s.models.duration)
        ),
        field_width + 119.,
    );
    let field = ui::area(
        r.right() - field_width - 119.,
        r.center().y - 16.,
        field_width,
        32.,
    );
    detail::filter(ui, v, field, "Filter songs");
    let ids = library::filtered(ui, v, &all);
    if ui::icon_btn(
        ui,
        ui::area(r.right() - 99., r.center().y - 15., 30., 30.),
        "shuffle",
        "Shuffle",
        false,
        v.tint,
    )
    .clicked()
    {
        library::play(
            v,
            &ids,
            ctx.clone(),
            true,
            !v.views.filter.trim().is_empty(),
        );
    }
    let is_filtered = !v.views.filter.trim().is_empty();
    let playing = !is_filtered && library::is_now(v, &ctx) && v.st.playing;
    if ui::play_big(
        ui,
        Rect::from_center_size(pos2(r.right() - 28., r.center().y), Vec2::splat(56.)),
        playing,
        v.tint,
    )
    .clicked()
    {
        library::play(v, &ids, ctx.clone(), false, is_filtered);
    }
    ui.add_space(35.);
    if let Some(id) = v.views.reveal_id.take()
        && let Some(i) = ids.iter().position(|s| s == &id)
    {
        v.views.selection.choose(i, false, false);
        v.views.selection.reveal = Some(i);
    }
    super::track_rows(ui, v, &ids, ctx, Columns::Added);
    if ids.is_empty() {
        library::note(ui, "No songs match");
    }
}
