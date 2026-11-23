use crate::{route::Route, smoke::Case};

pub const HOST: Case = Case {
    name: "host-library",
    prepare: |v| {
        v.go(Route::Settings);
        v.pick_folders();
        v.platform.phone.request();
    },
    ready: |v| {
        v.views.history.current == Route::Settings
            && v.platform.picker.is_none()
            && !v.platform.phone.busy()
            && (v.s.setting("phoneStats") == false || v.s.field("phone")["at"].is_number())
    },
};
