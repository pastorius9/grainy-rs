# grainy-app: the remaining work packages

What the Python Grainy 0.6.8 can do and the Rust app cannot yet, as packages one worker finishes in one run.
Read `crates/grainy-app/ARCHITECTURE.md` first: a package owns the files listed under **Owns** and adds one
registration line per command, panel, tool or dialog (commands keep the shortcut table's ids; the menus already
list every oracle item in order, a package flips its items from `""` to its command id). Strings stay the
oracle's Korean source text through `tr`. Tests: the package's own model tests against oracle goldens (extend
`tools/golden/gen_app.py`, or run the oracle's own methods on the stand-in `Window` there) and at least one
snapshot in both languages.

**Done (runs 1–3):** window layout, menus in the oracle's order, shortcut table, screen modes (F, Tab,
Shift+Tab), view (fit, 1:1, Ctrl+wheel, pan, clipping overlay, before `\`), histogram, every slider of every
panel, panel switches and dots, film list, DRT looks, colour filter, film balance, the library on disk
(catalogue document: listings with filters/search/folders/collections/sort/stacks equal to the oracle's, commits
and undo/redo with the oracle's history rows and `undo:{id}`, undo across sessions, history list and restore,
ratings/flags/colour labels keys and menus, virtual copies, import in place with thumbnails in the shared
cache, removal from the library, read-only opening of the Python Grainy's library), copy/paste of edits, export
dialog and export job, automatic tone / colour / negative as jobs, RAW white-balance controls for RAW files,
debug overlay (`--debug`, 설정 → 디버그 정보 표시).

Order: most used first. Dependencies name earlier packages or other crates.

---

### 1. Library column and session state

* **Ports:** app.py `refresh_folder_tree` (missing/unscanned marks, counts direct or total by the subfolder
  check box, expansion saved), `filter_folder_tree`, `folder_context_menu` (폴더 가져오기…, 폴더 목록 새로 고침, 폴더
  동기화 · 새 사진 가져오기 → `sync_folder`, 탐색기에서 열기, 상위 폴더 표시, 카탈로그에서 폴더 제거… →
  `folder_panel.remove` with its backup), `remove_folder` (the − button), `save_folder_state` /
  `restore_folder_selection` (preference `folder_panel`: folder, subfolders, expanded, current photo),
  preference `browser_view` (sort) and `develop_panels` (solo, expanded), `activate`'s metadata refresh
  (`catalog.update(metadata=…)` after the load), `prefetch_neighbors`, `import_refresh_timer` (the list grows
  during an import), `cancel_import` (status-bar button 가져오기 중단), the 읽을 수 없는 사진 box, `import_reason`,
  library lock (`main.py` `QLockFile` 이 사진 라이브러리가 이미 열려 있습니다).
* **Owns:** `library.rs`, `commands/library_column.rs`, `dialogs/import_errors.rs`, `session_state.rs`;
  additive `Document` methods (`remove_folder`, `save_view_state`) in `document/catalog.rs`.
* **Depends:** grainy-catalog (`remove_folder`, preferences), grainy-library `folder_sync` (`imported`,
  `exclude`).
* **Tests:** gen_app script steps for folder removal and the saved preferences (tables equal); snapshot of a tree
  with a missing folder; a reopened library restores folder, photo and mode.

### 2. Crop tool

* **Ports:** widgets.py `PhotoView` crop mode (`set_crop_mode`, `crop_values`, `drag_crop`, `resize_crop`,
  `hit_handle`, `limited_crop`, `set_aspect`, `reset_crop`, `accept_crop`, `draw_crop`, rotate outside the
  frame, `dragged_angle`), app.py `toggle_crop`, `end_crop`, `cancel_crop`, `crop_changed`, `set_straighten`
  (`straighten.refit`), `auto_border_crop` (`scan_border.crop_for`, job), `auto_straighten` (`upright.estimate`
  level, job), `rotate_photo`, flip; the uncropped render while cropping (`use_crop='all'`, `canvas_rect`).
* **Owns:** `tools/crop.rs`, `panels/crop.rs`, `commands/crop.rs`; an additive `PreviewJob::crop` option in
  `render.rs` / `worker.rs` (`Crop::All`).
* **Depends:** `imagespace`; grainy-geometry `straighten::{shrink, limit, refit}`, grainy-engine
  `auto::{scan_border_crop, upright_for}`.
* **Tests:** crop maths against goldens of `limited_crop` / `refit` / `set_aspect`; scripted drags through the
  tool's `pointer` (no window); snapshots of the crop frame straightened and with an aspect.

### 3. Tone curve and channel curves, targeted adjustment

* **Ports:** widgets.py `ToneCurve` (drag, Ctrl+click add, right-click delete, double-click reset, endpoints
  vertical only), studio.py channel curve (`rgb_curve_changed`, R/G/B), targeted.py `TargetedAdjust`
  (`toggle`, `started`, `dragged`, `finished`, `curve_input`, `drag_curve`; labels 사진에서 톤 커브 조정).
* **Owns:** `widgets/curve.rs` (new folder for widgets of this package), `panels/curve.rs`, `tools/targeted.rs`.
* **Depends:** grainy-tone curve evaluation for the drawn curve.
* **Tests:** point edits against the oracle's `ToneCurve` logic (goldens of the point lists after scripted
  clicks); `curve_input` golden; snapshots.

### 4. White-balance and film-base pickers

* **Ports:** app.py `toggle_wb`, `sample_wb`, `sample_wb_area`, `toggle_negative_base`, `sample_negative_base`;
  eyedropper.py `mean_colour`, `area_block`; widgets.py sample mode (click vs rectangle, the loupe ×6 with R G B,
  Esc); RAW path `rawcolor.neutral_white` (raw_neutral, raw_mode `sample`).
* **Owns:** `tools/eyedropper.rs`, `commands/pickers.rs` (`develop.white_balance` W).
* **Depends:** `imagespace::CoordinateMap::source_point`; grainy-raw `rawcolor`, grainy-film `negative::base_from`.
* **Tests:** goldens of the sampled settings for clicks and rectangles on the standard inputs (JPEG and a RAW
  case from golden/rawload); snapshot of the loupe.

### 5. Presets

* **Ports:** app.py `refresh_presets` (film looks by maker and kind `film_look.by_maker` / `short_name`, 기타
  프리셋 with `engine.PRESETS` and `catalog.presets()`, Adobe report tooltips), `apply_preset`
  (`adobe_preset.resolve`, retargets, geometry kept), `save_preset`; manager.py `import_preset` (.json, .xmp,
  .lrtemplate, review box 프리셋 추가, test develop), `export_preset`, `delete_preset`.
* **Owns:** `panels/presets.rs` (the library column's fold-out body through a hook in `library.rs`: one line),
  `commands/presets.rs`, `dialogs/preset_review.rs`.
* **Depends:** grainy-adobe (preset reading/translation), grainy-catalog presets.
* **Tests:** gen_app script steps save/apply/delete preset (tables equal); Adobe preset import against
  grainy-adobe goldens; snapshot of the tree.

### 6. Metadata panel, keywords and the metadata dialogs

* **Ports:** app.py metadata panel (rating and flag combos, keywords `save_keywords`, BATCH EDIT copy/paste with
  붙여넣기에 크롭·회전도 포함 `copy_crop_changed`, FILE info, 파일 위치 열기 `show_file`); manager.py `metadata`
  dialog (제목, 설명, 촬영자, 저작권, 장소, 도시, 국가, 촬영 시각, 위도, 경도, 키워드), `shift_date`,
  `refresh_metadata` (job), `keyword_tree`.
* **Owns:** `panels/metadata.rs`, `dialogs/metadata.rs`, `dialogs/keyword_tree.rs`, `commands/metadata.rs`.
* **Depends:** grainy-catalog `set_user_metadata`, grainy-codecs `read_metadata`.
* **Tests:** gen_app steps (keywords, metadata dialog values, date shift) with equal tables; snapshots.

### 7. Colour mixer and point colour

* **Ports:** app.py mixer panel (8 colours, target 색상/채도/밝기, `hsl_changed`, `update_hsl`), studio.py point
  colour (`pick_point_color` job with `point_color.sample`, points combo, six sliders, remove, 선택 색상 범위
  표시 overlay `point_color.preview`), targeted HSL (`drag_hsl`, `hsl_weights`).
* **Owns:** `panels/mixer.rs`, `tools/point_color.rs`, the HSL part of `tools/targeted.rs` (package 3 creates it).
* **Depends:** 3 (targeted tool), grainy-colorgrade `point_color`.
* **Tests:** goldens of sampled points and HSL drags; snapshots.

### 8. Masks: model, panel and gradients

* **Ports:** studio.py `build_masks` (새 마스크 만들기 menu, tree with layers/components, 추가/빼기, 반전, 오버레이 표시
  O, mask menu: 이름 바꾸기…, 복제, 숨기기, 반전, 교차, 위/아래로, 삭제; component items), `create_mask`,
  `extend_mask`, `tree_selected`, `layer_property`, `rename_layer`, `remove_layer`, `duplicate_layer`,
  `move_layer`, `duplicate_component`, `move_component`, `drop_fresh_layer`, `mask_shortcut` (K, M, Shift+M),
  linear/radial gradients (`gradient_shape`, `radial_from_screen`, `radial_on_screen`, `component_handles`,
  `edit_component_handles`, `radial_turned`), the mask adjustments (양, 톤, 색상, 효과, 세부), overlay
  (`overlay_layer`, `pause_overlay`, red mask preview from `processing`).
* **Owns:** `panels/masks.rs`, `tools/masks.rs`, `commands/masks.rs` (`mask.*` ids of the shortcut table),
  `masks/` (model helpers).
* **Depends:** `imagespace` (all of it), grainy-local (mask evaluation for the overlay).
* **Tests:** component JSON after scripted pointer drags equal to the oracle's (run `StudioTools` methods on the
  stand-in in gen_app); snapshots with a radial and a linear mask and the overlay.

### 9. Masks: brush, colour and luminance ranges, sky

* **Ports:** studio.py brush (`stroke`, `preview_stroke`, `make_component`, flow, density, 밝기 제한
  `brush_range_changed`, 자동 마스크), `pick_range` (job, `range_mask.sample_selection`), the 범위 다듬기 editor,
  `add_sky` (sky editor 민감도, 경계 흐림), `add_luma`; widgets.py brush cursor and `[` `]` sizes (`mask.brush_size`).
* **Owns:** `tools/brush.rs`, `tools/range_pick.rs`, `panels/masks_ranges.rs` (a sub-body the masks panel calls).
* **Depends:** 8; grainy-local, grainy-sky.
* **Tests:** stroke components against gen_app goldens; range samples against `range_mask` goldens; snapshots.

### 10. Masks: local curves, HSL, point colours and the component dialog

* **Ports:** studio.py fold-out 커브 · 색상 혼합 · 선택 색상 (local curve RGB/R/G/B, local HSL, `local_point_*`,
  `pick_point_color(local=True)`), mask_dialog.py `MaskComponentDialog` (every field per kind,
  `result_component`).
* **Owns:** `panels/masks_local.rs`, `dialogs/mask_component.rs`.
* **Depends:** 8, 3, 7.
* **Tests:** dialog results against the oracle's `result_component` on the stand-in; snapshots.

### 11. Healing

* **Ports:** studio.py `build_retouch` (복제 도장, 힐링 브러시 Q, 먼지·흠집 제거, 적목 보정; Alt+click source;
  `make_retouch`; staged heal with the red coverage overlay, 실행 · 표시한 곳 제거 Enter → `apply_healing`, 마지막
  표시 취소, 표시 모두 취소 Esc; 복구 동작 N개, 마지막 복구 제거, 도구 끝내기; 힐링 원본 자동 선택으로 복귀
  `automatic_healing_source`), sizes and feather.
* **Owns:** `panels/healing.rs`, `tools/healing.rs`, `commands/healing.rs`.
* **Depends:** `imagespace`; grainy-local (retouch, inpaint).
* **Tests:** retouch operations after scripted strokes equal the oracle's; snapshot of a staged heal.

### 12. Sync and copy settings

* **Ports:** manager.py `EDIT_GROUPS`, `choose_copy` (복사·동기화 항목), `sync_selected` (동기화 button, 현재 보정 →
  선택 사진), `toggle_auto_sync` / `after_commit` (Auto Sync), `edit_photo` with `rawcolor.retarget` and
  `optics.retarget`; workflow_ui.py `previous_edits` (Ctrl+Alt+V, `develop.paste_previous`).
* **Owns:** `commands/sync.rs`, `dialogs/copy_items.rs`; `Document::edit_photos(ids, settings, label)`.
* **Depends:** grainy-service `edits` (copy keys, carried panels), a retarget with `Fitting` once lens profiles
  exist.
* **Tests:** gen_app steps (choose items, sync, auto sync during commits, paste previous): tables equal.

### 13. Export completion

* **Ports:** app.py `quick_export` (small / large / original / previous, Ctrl+E `file.quick_export`),
  `quick_export.preset` / `previous` / `PREFERENCE`, the 원본 + 설정 format (XMP sidecar), `start_export`'s
  `workflow_data.record_exports` and the failure box; workflow_ui.py `show_exports` (내보내기 기록…),
  `reset_exports`; photo_actions 빠른 내보내기 submenu.
* **Owns:** `commands/export.rs` (moves `file.export` there), `dialogs/export.rs`, `dialogs/export_history.rs`.
* **Depends:** grainy-library `quick_export`, `xmp`; grainy-catalog `workflow::{record_exports, export_history}`.
* **Tests:** export records equal the oracle's for a scripted export; files byte-identical to `grainy-cli` ones.

### 14. Photo context menu and photo actions

* **Ports:** photo_actions.py `build_menu` (every item and its disabled states), `edit_selected`,
  `transform_selected` (turns, flips, 카메라 방향으로 복원 `camera_orientation`), `reset_selected`, 흑백으로 전환 /
  컬러로 전환, 현상에서 크게 보기, 탐색기에서 폴더 열기.
* **Owns:** `menus_photo.rs` (replaces `photo_menu` in `view.rs`: one line there), `commands/photo_actions.rs`.
* **Depends:** 12 (`edit_photos`), 13, 15, 16 for the submenus they own (items stay disabled until then).
* **Tests:** gen_app steps for each transform on a selection (tables equal); snapshot of the open menu.

### 15. Collections and filters

* **Ports:** manager.py `FilterDialog` (상세 필터 and 스마트 컬렉션 규칙, every field, existing complex rules
  `smart_rules.describe`), `build_collections` (tree with ◈ smart marks and nesting), `new_collection`,
  `smart_collection`, `add_to_collection`, `remove_collection`, `edit_collection`, `remove_from_collection`,
  `clear_filter`, `refresh_timed_collection` (60 s); workflow_ui.py `quick_toggle` (B, `cull.quick_collection`),
  `quick_show`, `containing_collection`.
* **Owns:** `dialogs/filter.rs`, `panels/collections.rs` (the fold-out body through one line in `library.rs`),
  `commands/collections.rs`.
* **Depends:** grainy-catalog collections, grainy-adobe smart rules.
* **Tests:** listings with every filter field equal `library_query` (extend gen_app's LISTINGS); collection steps
  with equal tables.

### 16. Stacks, snapshots and virtual copies

* **Ports:** manager.py `stack`, `unstack`, `toggle_stacks` (스택 접어서 보기), `snapshot`, `restore_snapshot`;
  workflow_ui.py `stack_cover`, `stack_move`; the stack badge of the grid.
* **Owns:** `commands/stacks.rs`, `dialogs/snapshots.rs`.
* **Depends:** grainy-catalog `workflow::stack_order`.
* **Tests:** gen_app steps (tables equal, collapsed listings equal).

### 17. Compare and reference views

* **Ports:** compare_view.py `CompareLayout` (보정 전·후 / 여러 이미지, 좌우/상하, 바꾸기, 비교 닫기, `pick`,
  `before_settings`, `follow`, click swaps, double-click closes, 400 ms sync), manager.py `compare` (선택 사진
  모아보기, up to 12), reference_view.py `ReferenceWindow` (참조 사진, 두 번째 화면) and `render_photo`.
* **Owns:** `views/compare.rs`, `dialogs/survey.rs`, `dialogs/reference.rs`, `commands/compare.rs`
  (`view.compare`, `view.compare_original` stays); a second preview lane in `worker.rs` (additive).
* **Depends:** the renderer.
* **Tests:** `before_settings` and `pick` against the oracle; snapshots side by side.

### 18. Batch workflow (사진 작업)

* **Ports:** workflow_ui.py `auto_tone` / `auto_color` / `auto_white` for the selection (`command_auto_values`,
  `photo_service.auto_values`, all or nothing, progress box), `invert_negatives`, `invert_roll`, `border_crop`,
  `update_process`, `match_exposure`, metadata presets (`save_metadata`, `apply_metadata`, `delete_metadata`),
  shortcut keyword (`set_keyword`, `apply_keyword`).
* **Owns:** `commands/workflow.rs`, `dialogs/metadata_preset.rs`, `dialogs/progress.rs`.
* **Depends:** grainy-service `auto`, grainy-engine `auto`, grainy-catalog `workflow`.
* **Tests:** batches against grainy-service goldens and gen_app tables.

### 19. Transform and upright

* **Ports:** studio.py transform panel (자동 / 수평 / 수직 / 전체 원근 jobs `auto_upright`, status label, 자동 정렬
  초기화 `clear_upright`), guided upright (`start_guides`, four clicks, convexity and area check, 가이드 원근
  초기화).
* **Owns:** `panels/transform.rs`, `tools/guides.rs`.
* **Depends:** grainy-engine `auto::upright_for`, `imagespace`.
* **Tests:** guides from scripted clicks equal the oracle's; snapshots.

### 20. Camera calibration, DCP profiles and RAW import defaults

* **Ports:** studio.py `change_working`, `load_dcp`, `apply_dcp_record`, `clear_dcp`, `camera_profile`,
  `browse_dcp`, `display_raw_white`, `change_raw_mode`, `raw_temperature_changed`; profile_dialog.py
  `ProfileDialog` (search, favourites, folders, scan job) and `RawDefaultsDialog` (rules, scope, groups, ISO
  anchors).
* **Owns:** `panels/calibration.rs`, `dialogs/profiles.rs`, `dialogs/raw_defaults.rs`, `commands/raw.rs`.
* **Depends:** grainy-raw (`dcp`, `raw_defaults`, `profile_library`).
* **Tests:** rules and resolved settings against grainy-raw goldens; snapshots.

### 21. Lens correction

* **Ports:** studio.py `auto_lens`, `choose_lens`, `apply_lens`, profile check boxes and their enabling, manual
  lens `save_lens` / `load_lens`; lens_dialog.py `LensDialog`.
* **Owns:** `panels/lens.rs`, `dialogs/lens.rs`.
* **Depends:** grainy-lens (open: the owner's decision on the lensfun database); the engine's lens model.
* **Tests:** profiles from metadata equal `optics.match_metadata`; develops with a profile against the oracle.

### 22. File management and catalogue maintenance

* **Ports:** manager.py `rename`, `move`, `move_selected_folder`, `remove(True)` (원본을 휴지통으로, trash crate),
  `relink`, `duplicates` (job, tree dialog), `smart_previews` (offline packets), `backup`, `restore`,
  `optimize_catalog`, `integrity`, `relink_folder`; folder_panel.py `refresh` (5 s rename detection),
  `recover`, `locate`, `apply_many`; preview_queue.py (선택 사진 미리보기 다시 만들기, 일괄 미리보기 중단);
  maintenance_dialog.py `MaintenanceDialog`; offline editing (`preview_store.load_offline` when the original is
  missing).
* **Owns:** `commands/files.rs`, `dialogs/maintenance.rs`, `dialogs/duplicates.rs`, `folders.rs`; the offline
  source in `worker.rs` (additive).
* **Depends:** grainy-library (all of it), grainy-catalog `preview::{read_offline, write_offline, load_offline}`.
* **Tests:** grainy-library scenarios through the commands (tables and files equal); originals never touched.

### 23. Display colour management and photo background

* **Ports:** display_color.py (`DisplayTarget`, `DisplayColor`, status button 색상 · 자동/수동/해제/확인 필요,
  `DisplayDialog`), display_profiles.py, monitor ICC menu items, thumbnails in the monitor profile; app.py
  `set_view_background` (사진 배경 menu, view right-click).
* **Owns:** `display.rs`, `dialogs/display.rs`, `commands/display.rs`; the output profile of `PreviewJob`
  (additive).
* **Depends:** grainy-cms (`SystemProfiles`).
* **Tests:** frames with a monitor profile equal `engine.output_rgb`; snapshots.

### 24. Import copy, watched folders and folder sync

* **Ports:** import_copy.py `dialog` and its job (plan, execute, verify, backup copy, keywords, preset,
  duplicates), extras.py `watch` / `unwatch` / `watch_scan`, folder_sync.py (auto import every 120 s, setting
  등록한 폴더의 새 사진 자동으로 가져오기), `apply_import_extras`.
* **Owns:** `dialogs/import_copy.rs`, `commands/import.rs`, `folder_watch.rs`.
* **Depends:** grainy-library `import_copy`, `folder_sync`.
* **Tests:** grainy-library scenarios through the dialog's model; nothing overwritten.

### 25. Dust detection dialogs

* **Ports:** dust_dialog.py (`prepare_source`, `DustView`, `DustDialog` options, detection job, candidates,
  manual spots and scratches, repair preview, apply), dust_batch_dialog.py (sessions, scope, tree, sequential
  detection, review, apply, restore).
* **Owns:** `dialogs/dust.rs`, `dialogs/dust_batch.rs`.
* **Depends:** 11; grainy-dust (store, batch).
* **Tests:** detection lists and operations equal grainy-dust goldens; snapshots.

### 26. Window, settings and the small dialogs

* **Ports:** screen_modes.py `toggle_lights` (L, `view.lights`), splitter sizes; language_dialog.py (first run,
  window rebuild keeping photo, mode, panels: desktop_session.py `change_language`); 설정 menu: GPU 가속 사용
  (`set_gpu`), 새 버전 자동 확인, 업데이트 확인…, 이번 버전의 달라진 점… (whats_new.py), Grainy 정보… (`show_about`),
  Grainy 배우기 (web link), 사용법 / 구현 범위 (`show_help`); empty-state text of the view; preference
  `drt_advanced`.
* **Owns:** `commands/settings.rs`, `dialogs/language.rs`, `dialogs/whats_new.rs`, `dialogs/about.rs`.
* **Depends:** grainy-platform (`whats_new`, `preferences`, `version`).
* **Tests:** snapshots; preferences written to the Rust app's own settings file only.

### 27. GPU preview

* **Ports:** native_gpu.py `set_enabled` / `status` / warm-up; the GPU path of the preview.
* **Owns:** a GPU renderer behind `Renderer` (`gpu_worker.rs`).
* **Depends:** grainy-gpu (merged, not wired).
* **Tests:** frames against the CPU worker within grainy-gpu's tolerances; timing test.

### 28. XMP sidecars and the GPS map

* **Ports:** manager.py `write_xmp` / `read_xmp` (`.luma-copy-{id}.xmp` for virtual copies); extras.py `map`
  (`MapView` on `assets/world.geojson`, markers, open on double-click).
* **Owns:** `commands/xmp.rs`, `dialogs/map.rs`.
* **Depends:** grainy-library `xmp`.
* **Tests:** sidecars byte-equal grainy-library goldens; map snapshot.

### 29. Lightroom import

* **Ports:** lightroom_import_dialog.py `begin_import` (inspect job, review box, convert edits/history/snapshots,
  progress, report), extras.py `lightroom_report`.
* **Owns:** `dialogs/lightroom.rs`, `commands/lightroom.rs`.
* **Depends:** grainy-adobe (including `adobe_history.transfer`), 22's `MaintenanceDialog`.
* **Tests:** a generated `.lrcat` imported: tables equal the oracle's import.

### 30. AI connection and Ask AI

* **Ports:** mcp_dialog.py `McpDialog` and mcp_service.py `McpWindowState` (status button AI 작업 중단, the window
  side of every tool), ai_dialog.py `AiDialog` with ai_programs / ai_auth / ai_install, `--ask-ai`.
* **Owns:** `ai/` (the MCP host on the jobs service), `dialogs/mcp.rs`, `dialogs/ask_ai.rs`, `commands/ai.rs`.
* **Depends:** grainy-mcp, grainy-service batches; every command is callable by id (the AI tools call commands).
* **Tests:** grainy-mcp handle cases through the window's host; snapshots.

### 31. Feedback, updates, safe start, cloud backup

* **Ports:** feedback.py `FeedbackDialog`, updater.py `UpdateController` (first-run question, daily check, offer,
  install, finish), safe_start.py (crash marker, safe mode), cloud_backup.py (구글 드라이브 백업 dialog, 30-minute
  job, final upload on close), file_access.py `guide` / `check_folders` (folder permissions).
* **Owns:** `dialogs/feedback.rs`, `dialogs/update.rs`, `dialogs/cloud_backup.rs`, `startup.rs`.
* **Depends:** grainy-platform (`feedback`, `updater`, `safe_start`), an HTTP client (owner's choice).
* **Tests:** platform goldens through the dialogs' models; nothing sent in tests.

### 32. HDR display and video (dormant in 0.6.8)

* **Ports:** hdr_display.py and the HDR controls of app.py (`HDR_FEATURE = False` in the oracle: hidden), the video
  rows older catalogues may hold (`video_color`, `video_time`, `VIDEO_EXTENSIONS`; 0.6.8 does not import videos).
* **Owns:** `hdr.rs`, `video.rs`. Only when the owner switches HDR on or wants video.
