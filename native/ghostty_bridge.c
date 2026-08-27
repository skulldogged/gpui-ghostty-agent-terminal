#include "ghostty_bridge.h"

#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>

struct SpikeTerminal {
  GhosttyTerminal terminal;
  GhosttyRenderState render;
  GhosttyRenderStateRowIterator rows;
  GhosttyRenderStateRowCells cells;
  GhosttyMouseEncoder mouse_encoder;
  GhosttyMouseEvent mouse_event;
  GhosttySelectionGesture selection_gesture;
  GhosttySelectionGestureEvent selection_press;
  GhosttySelectionGestureEvent selection_drag;
  GhosttySelectionGestureEvent selection_release;
  GhosttySelectionGestureEvent selection_autoscroll;
  uint8_t* pty_response;
  size_t pty_response_len;
  size_t pty_response_capacity;
  bool pty_response_failed;
};

static bool success(GhosttyResult result) { return result == GHOSTTY_SUCCESS; }

static void write_pty(GhosttyTerminal terminal, void* userdata,
                      const uint8_t* data, size_t len) {
  (void)terminal;
  SpikeTerminal* spike = userdata;
  if (spike == NULL || len == 0 || spike->pty_response_failed) return;
  if (data == NULL || len > SIZE_MAX - spike->pty_response_len) {
    spike->pty_response_failed = true;
    return;
  }

  const size_t required = spike->pty_response_len + len;
  if (required > spike->pty_response_capacity) {
    size_t capacity = spike->pty_response_capacity;
    if (capacity == 0) capacity = 64;
    while (capacity < required) {
      if (capacity > SIZE_MAX / 2) {
        capacity = required;
        break;
      }
      capacity *= 2;
    }

    uint8_t* response = realloc(spike->pty_response, capacity);
    if (response == NULL) {
      spike->pty_response_failed = true;
      return;
    }
    spike->pty_response = response;
    spike->pty_response_capacity = capacity;
  }

  memcpy(spike->pty_response + spike->pty_response_len, data, len);
  spike->pty_response_len = required;
}

SpikeTerminal* spike_terminal_new(uint16_t cols, uint16_t rows, size_t scrollback) {
  SpikeTerminal* spike = calloc(1, sizeof(SpikeTerminal));
  if (spike == NULL) return NULL;

  GhosttyTerminalOptions options = {
      .cols = cols,
      .rows = rows,
      .max_scrollback = scrollback,
  };
  if (!success(ghostty_terminal_new(NULL, &spike->terminal, options)) ||
      !success(ghostty_terminal_set(spike->terminal,
                                    GHOSTTY_TERMINAL_OPT_USERDATA, spike)) ||
      !success(ghostty_terminal_set(spike->terminal,
                                    GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                                    (const void*)write_pty)) ||
      !success(ghostty_render_state_new(NULL, &spike->render)) ||
      !success(ghostty_render_state_row_iterator_new(NULL, &spike->rows)) ||
      !success(ghostty_render_state_row_cells_new(NULL, &spike->cells)) ||
      !success(ghostty_mouse_encoder_new(NULL, &spike->mouse_encoder)) ||
      !success(ghostty_mouse_event_new(NULL, &spike->mouse_event)) ||
      !success(ghostty_selection_gesture_new(NULL,
                                             &spike->selection_gesture)) ||
      !success(ghostty_selection_gesture_event_new(
          NULL, &spike->selection_press,
          GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS)) ||
      !success(ghostty_selection_gesture_event_new(
          NULL, &spike->selection_drag,
          GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG)) ||
      !success(ghostty_selection_gesture_event_new(
          NULL, &spike->selection_release,
          GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE)) ||
      !success(ghostty_selection_gesture_event_new(
          NULL, &spike->selection_autoscroll,
          GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK))) {
    spike_terminal_free(spike);
    return NULL;
  }
  return spike;
}

void spike_terminal_free(SpikeTerminal* spike) {
  if (spike == NULL) return;
  ghostty_selection_gesture_event_free(spike->selection_autoscroll);
  ghostty_selection_gesture_event_free(spike->selection_release);
  ghostty_selection_gesture_event_free(spike->selection_drag);
  ghostty_selection_gesture_event_free(spike->selection_press);
  ghostty_selection_gesture_free(spike->selection_gesture, spike->terminal);
  ghostty_mouse_event_free(spike->mouse_event);
  ghostty_mouse_encoder_free(spike->mouse_encoder);
  ghostty_render_state_row_cells_free(spike->cells);
  ghostty_render_state_row_iterator_free(spike->rows);
  ghostty_render_state_free(spike->render);
  ghostty_terminal_free(spike->terminal);
  free(spike->pty_response);
  free(spike);
}

int spike_terminal_write(SpikeTerminal* spike, const uint8_t* data, size_t len,
                         const uint8_t** response, size_t* response_len) {
  if (spike == NULL || (data == NULL && len > 0) || response == NULL ||
      response_len == NULL) {
    return GHOSTTY_INVALID_VALUE;
  }

  spike->pty_response_len = 0;
  spike->pty_response_failed = false;
  ghostty_terminal_vt_write(spike->terminal, data, len);
  if (spike->pty_response_failed) return GHOSTTY_OUT_OF_MEMORY;

  *response = spike->pty_response;
  *response_len = spike->pty_response_len;
  return GHOSTTY_SUCCESS;
}

int spike_terminal_resize(SpikeTerminal* spike, uint16_t cols, uint16_t rows,
                          uint32_t cell_width_px, uint32_t cell_height_px) {
  if (spike == NULL) return GHOSTTY_INVALID_VALUE;
  return ghostty_terminal_resize(spike->terminal, cols, rows, cell_width_px,
                                 cell_height_px);
}

static GhosttyColorRgb color_from_bytes(const uint8_t* color) {
  return (GhosttyColorRgb){.r = color[0], .g = color[1], .b = color[2]};
}

int spike_terminal_set_theme(SpikeTerminal* spike, const uint8_t* foreground,
                             const uint8_t* background, const uint8_t* cursor,
                             const uint8_t* ansi_palette) {
  if (spike == NULL || foreground == NULL || background == NULL ||
      cursor == NULL || ansi_palette == NULL) {
    return GHOSTTY_INVALID_VALUE;
  }

  GhosttyColorRgb palette[256];
  GhosttyResult result = ghostty_terminal_get(
      spike->terminal, GHOSTTY_TERMINAL_DATA_COLOR_PALETTE_DEFAULT, palette);
  if (!success(result)) return result;
  for (size_t index = 0; index < 16; index++) {
    palette[index] = color_from_bytes(&ansi_palette[index * 3]);
  }

  GhosttyColorRgb foreground_color = color_from_bytes(foreground);
  GhosttyColorRgb background_color = color_from_bytes(background);
  GhosttyColorRgb cursor_color = color_from_bytes(cursor);
  result = ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                                &foreground_color);
  if (!success(result)) return result;
  result = ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                                &background_color);
  if (!success(result)) return result;
  result = ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                                &cursor_color);
  if (!success(result)) return result;
  return ghostty_terminal_set(spike->terminal,
                              GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette);
}

int spike_terminal_encode_paste(SpikeTerminal* spike, uint8_t* data,
                                 size_t data_len, uint8_t* output,
                                 size_t output_len, size_t* output_written) {
  if (spike == NULL || (data == NULL && data_len > 0) ||
      (output == NULL && output_len > 0) || output_written == NULL) {
    return GHOSTTY_INVALID_VALUE;
  }

  bool bracketed = false;
  GhosttyResult result = ghostty_terminal_mode_get(
      spike->terminal, GHOSTTY_MODE_BRACKETED_PASTE, &bracketed);
  if (!success(result)) return result;

  return ghostty_paste_encode((char*)data, data_len, bracketed, (char*)output,
                               output_len, output_written);
}

static GhosttyResult scroll_viewport(SpikeTerminal* spike,
                                     GhosttyTerminalScrollViewport behavior,
                                     bool* changed) {

  GhosttyTerminalScrollbar before = {0};
  GhosttyResult result = ghostty_terminal_get(
      spike->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &before);
  if (!success(result)) return result;

  ghostty_terminal_scroll_viewport(spike->terminal, behavior);

  GhosttyTerminalScrollbar after = {0};
  result = ghostty_terminal_get(spike->terminal,
                                GHOSTTY_TERMINAL_DATA_SCROLLBAR, &after);
  if (!success(result)) return result;
  *changed = before.offset != after.offset;
  return GHOSTTY_SUCCESS;
}

int spike_terminal_scroll_to_bottom(SpikeTerminal* spike, bool* changed) {
  if (spike == NULL || changed == NULL) return GHOSTTY_INVALID_VALUE;
  return scroll_viewport(
      spike,
      (GhosttyTerminalScrollViewport){.tag = GHOSTTY_SCROLL_VIEWPORT_BOTTOM},
      changed);
}

static GhosttyResult selection_ref_at(SpikeTerminal* spike, uint16_t x,
                                      uint16_t y, GhosttyGridRef* out_ref) {
  GhosttyPoint point = {
      .tag = GHOSTTY_POINT_TAG_VIEWPORT,
      .value = {.coordinate = {.x = x, .y = y}},
  };
  GhosttyGridRef ref = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  *out_ref = ref;
  return ghostty_terminal_grid_ref(spike->terminal, point, out_ref);
}

static GhosttyResult selection_event_set(
    GhosttySelectionGestureEvent event,
    GhosttySelectionGestureEventOption option, const void* value) {
  return ghostty_selection_gesture_event_set(event, option, value);
}

static GhosttyResult install_selection(SpikeTerminal* spike,
                                       GhosttyResult result,
                                       GhosttySelection* selection,
                                       bool clear_on_no_value) {
  if (success(result)) {
    return ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_SELECTION, selection);
  }
  if (result == GHOSTTY_NO_VALUE && clear_on_no_value) {
    return ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
  }
  return result;
}

int spike_terminal_selection_event(SpikeTerminal* spike,
                                   const SpikeSelectionInput* input,
                                   bool* viewport_changed) {
  if (spike == NULL || input == NULL || viewport_changed == NULL) {
    return GHOSTTY_INVALID_VALUE;
  }
  *viewport_changed = false;
  if (input->type == 4) {
    ghostty_selection_gesture_reset(spike->selection_gesture, spike->terminal);
    return ghostty_terminal_set(spike->terminal,
                                GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
  }

  if (input->columns == 0 || input->cell_width == 0 ||
      input->screen_height == 0) {
    return GHOSTTY_INVALID_VALUE;
  }
  GhosttySelectionGestureGeometry geometry = {
      .columns = input->columns,
      .cell_width = input->cell_width,
      .padding_left = input->padding_left,
      .screen_height = input->screen_height,
  };
  GhosttySurfacePosition position = {
      .x = input->pointer_x,
      .y = input->pointer_y,
  };
  GhosttySelection selection = GHOSTTY_INIT_SIZED(GhosttySelection);
  GhosttyResult result = GHOSTTY_INVALID_VALUE;

  if (input->type == 0) {
    GhosttyGridRef ref;
    result = selection_ref_at(spike, input->x, input->y, &ref);
    if (!success(result)) return result;
    GhosttySelectionGestureBehavior behavior =
        input->click_count >= 3
            ? GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE
            : (input->click_count == 2
                   ? GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD
                   : GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL);
    GhosttySelectionGestureBehaviors behaviors = {
        .single_click = behavior,
        .double_click = behavior,
        .triple_click = behavior,
    };
    result = selection_event_set(spike->selection_press,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                                 &ref);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_press,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                                 &position);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_press,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS,
                                 &behaviors);
    if (!success(result)) return result;
    result = ghostty_selection_gesture_event(
        spike->selection_gesture, spike->terminal, spike->selection_press,
        &selection);
    return install_selection(spike, result, &selection, true);
  }

  if (input->type == 1) {
    GhosttyGridRef ref;
    result = selection_ref_at(spike, input->x, input->y, &ref);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_drag,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                                 &ref);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_drag,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                                 &position);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_drag,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                                 &geometry);
    if (!success(result)) return result;
    result = ghostty_selection_gesture_event(
        spike->selection_gesture, spike->terminal, spike->selection_drag,
        &selection);
    result = install_selection(spike, result, &selection, false);
    return result == GHOSTTY_NO_VALUE ? GHOSTTY_SUCCESS : result;
  }

  if (input->type == 2) {
    GhosttyGridRef ref;
    result = selection_ref_at(spike, input->x, input->y, &ref);
    if (!success(result)) return result;
    result = selection_event_set(spike->selection_release,
                                 GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                                 &ref);
    if (!success(result)) return result;
    result = ghostty_selection_gesture_event(
        spike->selection_gesture, spike->terminal, spike->selection_release,
        NULL);
    return result == GHOSTTY_NO_VALUE ? GHOSTTY_SUCCESS : result;
  }

  if (input->type == 3) {
    GhosttyTerminalScrollbar before = {0};
    result = ghostty_terminal_get(spike->terminal,
                                  GHOSTTY_TERMINAL_DATA_SCROLLBAR, &before);
    if (!success(result)) return result;
    GhosttyPointCoordinate viewport = {.x = input->x, .y = input->y};
    result = selection_event_set(
        spike->selection_autoscroll,
        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT, &viewport);
    if (!success(result)) return result;
    result = selection_event_set(
        spike->selection_autoscroll,
        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position);
    if (!success(result)) return result;
    result = selection_event_set(
        spike->selection_autoscroll,
        GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &geometry);
    if (!success(result)) return result;
    result = ghostty_selection_gesture_event(
        spike->selection_gesture, spike->terminal,
        spike->selection_autoscroll, &selection);
    result = install_selection(spike, result, &selection, false);
    if (!success(result) && result != GHOSTTY_NO_VALUE) return result;
    GhosttyTerminalScrollbar after = {0};
    result = ghostty_terminal_get(spike->terminal,
                                  GHOSTTY_TERMINAL_DATA_SCROLLBAR, &after);
    if (!success(result)) return result;
    *viewport_changed = before.offset != after.offset;
    return GHOSTTY_SUCCESS;
  }

  return GHOSTTY_INVALID_VALUE;
}

int spike_terminal_selection_text(SpikeTerminal* spike, uint8_t* output,
                                  size_t output_len,
                                  size_t* output_written) {
  if (spike == NULL || output_written == NULL ||
      (output == NULL && output_len > 0)) {
    return GHOSTTY_INVALID_VALUE;
  }
  GhosttyTerminalSelectionFormatOptions options =
      GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
  options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
  options.unwrap = true;
  options.trim = true;
  options.selection = NULL;
  return ghostty_terminal_selection_format_buf(
      spike->terminal, options, output, output_len, output_written);
}

static GhosttyResult encode_mouse_scroll(SpikeTerminal* spike,
                                         intptr_t delta,
                                         GhosttyMouseButton negative_button,
                                         GhosttyMouseButton positive_button,
                                         uint8_t* output, size_t output_len,
                                         size_t* output_written) {
  if (delta == 0) return GHOSTTY_SUCCESS;
  ghostty_mouse_event_set_button(
      spike->mouse_event, delta < 0 ? negative_button : positive_button);
  const size_t count =
      delta < 0 ? (size_t)(-(delta + 1)) + 1 : (size_t)delta;
  for (size_t index = 0; index < count; index++) {
    size_t written = 0;
    GhosttyResult result = ghostty_mouse_encoder_encode(
        spike->mouse_encoder, spike->mouse_event,
        (char*)output + *output_written, output_len - *output_written,
        &written);
    if (!success(result)) return result;
    *output_written += written;
  }
  return GHOSTTY_SUCCESS;
}

int spike_terminal_scroll(SpikeTerminal* spike, intptr_t delta_rows,
                          intptr_t delta_columns,
                          float pointer_x, float pointer_y,
                          uint32_t viewport_width, uint32_t viewport_height,
                          uint32_t cell_width, uint32_t cell_height,
                          uint16_t modifiers, uint8_t* output,
                          size_t output_len, size_t* output_written,
                          bool* viewport_changed) {
  if (spike == NULL || output_written == NULL || viewport_changed == NULL ||
      (output == NULL && output_len > 0) || cell_width == 0 ||
      cell_height == 0) {
    return GHOSTTY_INVALID_VALUE;
  }
  *output_written = 0;
  *viewport_changed = false;
  if (delta_rows == 0 && delta_columns == 0) return GHOSTTY_SUCCESS;

  bool mouse_tracking = false;
  GhosttyResult result = ghostty_terminal_get(
      spike->terminal, GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, &mouse_tracking);
  if (!success(result)) return result;

  if (mouse_tracking) {
    ghostty_mouse_encoder_setopt_from_terminal(spike->mouse_encoder,
                                                spike->terminal);
    GhosttyMouseEncoderSize encoder_size = {
        .size = sizeof(GhosttyMouseEncoderSize),
        .screen_width = viewport_width,
        .screen_height = viewport_height,
        .cell_width = cell_width,
        .cell_height = cell_height,
    };
    ghostty_mouse_encoder_setopt(spike->mouse_encoder,
                                 GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &encoder_size);
    ghostty_mouse_event_set_action(spike->mouse_event,
                                   GHOSTTY_MOUSE_ACTION_PRESS);
    ghostty_mouse_event_set_mods(spike->mouse_event, modifiers);
    ghostty_mouse_event_set_position(
        spike->mouse_event,
        (GhosttyMousePosition){.x = pointer_x, .y = pointer_y});

    result = encode_mouse_scroll(
        spike, delta_rows, GHOSTTY_MOUSE_BUTTON_FOUR,
        GHOSTTY_MOUSE_BUTTON_FIVE, output, output_len, output_written);
    if (!success(result)) return result;
    return encode_mouse_scroll(
        spike, delta_columns, GHOSTTY_MOUSE_BUTTON_SIX,
        GHOSTTY_MOUSE_BUTTON_SEVEN, output, output_len, output_written);
  }

  GhosttyTerminalScreen screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
  result = ghostty_terminal_get(spike->terminal,
                                GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
  if (!success(result)) return result;
  bool alternate_scroll = false;
  result = ghostty_terminal_mode_get(spike->terminal, GHOSTTY_MODE_ALT_SCROLL,
                                     &alternate_scroll);
  if (!success(result)) return result;
  if (screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE && alternate_scroll) {
    bool application_cursor_keys = false;
    result = ghostty_terminal_mode_get(spike->terminal, GHOSTTY_MODE_DECCKM,
                                       &application_cursor_keys);
    if (!success(result)) return result;
    const char* sequence = delta_rows < 0
                               ? (application_cursor_keys ? "\x1bOA" : "\x1b[A")
                               : (application_cursor_keys ? "\x1bOB" : "\x1b[B");
    const size_t count = delta_rows < 0 ? (size_t)(-(delta_rows + 1)) + 1
                                        : (size_t)delta_rows;
    if (count > output_len / 3) return GHOSTTY_OUT_OF_SPACE;
    for (size_t index = 0; index < count; index++) {
      memcpy(output + *output_written, sequence, 3);
      *output_written += 3;
    }
    return GHOSTTY_SUCCESS;
  }

  return scroll_viewport(
      spike,
      (GhosttyTerminalScrollViewport){
          .tag = GHOSTTY_SCROLL_VIEWPORT_DELTA,
          .value = {.delta = delta_rows},
      },
      viewport_changed);
}

static void set_color(uint8_t* r, uint8_t* g, uint8_t* b, GhosttyColorRgb color) {
  *r = color.r;
  *g = color.g;
  *b = color.b;
}

int spike_terminal_snapshot(SpikeTerminal* spike, bool force_full,
                            SpikeSnapshot* snapshot, SpikeCell* cells,
                            size_t capacity) {
  if (spike == NULL || snapshot == NULL || (cells == NULL && capacity > 0)) {
    return GHOSTTY_INVALID_VALUE;
  }
  GhosttyResult result = ghostty_render_state_update(spike->render, spike->terminal);
  if (!success(result)) return result;

  memset(snapshot, 0, sizeof(*snapshot));
  ghostty_render_state_get(spike->render, GHOSTTY_RENDER_STATE_DATA_COLS,
                           &snapshot->cols);
  ghostty_render_state_get(spike->render, GHOSTTY_RENDER_STATE_DATA_ROWS,
                           &snapshot->rows);

  GhosttyRenderStateDirty dirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
  result = ghostty_render_state_get(
      spike->render, GHOSTTY_RENDER_STATE_DATA_DIRTY, &dirty);
  if (!success(result)) return result;
  snapshot->full = force_full || dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL;

  bool cursor_in_viewport = false;
  bool cursor_mode_visible = false;
  ghostty_render_state_get(spike->render,
                           GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
                           &cursor_in_viewport);
  ghostty_render_state_get(spike->render,
                           GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
                           &cursor_mode_visible);
  snapshot->cursor_visible = cursor_in_viewport && cursor_mode_visible;
  if (cursor_in_viewport) {
    ghostty_render_state_get(spike->render,
                             GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
                             &snapshot->cursor_x);
    ghostty_render_state_get(spike->render,
                             GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
                             &snapshot->cursor_y);
  }

  result = ghostty_terminal_mode_get(
      spike->terminal, GHOSTTY_MODE_BRACKETED_PASTE,
      &snapshot->bracketed_paste);
  if (!success(result)) return result;
  bool alternate_screen = false;
  result = ghostty_terminal_mode_get(spike->terminal, GHOSTTY_MODE_ALT_SCREEN,
                                     &alternate_screen);
  if (!success(result)) return result;
  bool alternate_screen_save = false;
  result = ghostty_terminal_mode_get(
      spike->terminal, GHOSTTY_MODE_ALT_SCREEN_SAVE, &alternate_screen_save);
  if (!success(result)) return result;
  snapshot->alternate_screen = alternate_screen || alternate_screen_save;
  GhosttySelection active_selection = GHOSTTY_INIT_SIZED(GhosttySelection);
  result = ghostty_terminal_get(spike->terminal,
                                GHOSTTY_TERMINAL_DATA_SELECTION,
                                &active_selection);
  snapshot->selection_active = success(result);
  if (!success(result) && result != GHOSTTY_NO_VALUE) return result;

  GhosttyString title = {0};
  result = ghostty_terminal_get(spike->terminal, GHOSTTY_TERMINAL_DATA_TITLE,
                                &title);
  if (!success(result)) return result;
  size_t title_len = title.len;
  if (title_len > sizeof(snapshot->title)) {
    title_len = sizeof(snapshot->title);
  }
  if (title_len > 0 && title.ptr != NULL) {
    memcpy(snapshot->title, title.ptr, title_len);
  }
  snapshot->title_len = (uint16_t)title_len;

  GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
  result = ghostty_render_state_colors_get(spike->render, &colors);
  if (!success(result)) return result;
  set_color(&snapshot->default_fg_r, &snapshot->default_fg_g,
            &snapshot->default_fg_b, colors.foreground);
  set_color(&snapshot->default_bg_r, &snapshot->default_bg_g,
            &snapshot->default_bg_b, colors.background);

  result = ghostty_render_state_get(spike->render,
                                    GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                                    &spike->rows);
  if (!success(result)) return result;

  size_t count = 0;
  uint16_t y = 0;
  while (ghostty_render_state_row_iterator_next(spike->rows)) {
    bool row_dirty = false;
    result = ghostty_render_state_row_get(
        spike->rows, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &row_dirty);
    if (!success(result)) return result;
    if (!snapshot->full &&
        (dirty != GHOSTTY_RENDER_STATE_DIRTY_PARTIAL || !row_dirty)) {
      y++;
      continue;
    }

    GhosttyRow raw_row = 0;
    result = ghostty_render_state_row_get(
        spike->rows, GHOSTTY_RENDER_STATE_ROW_DATA_RAW, &raw_row);
    if (!success(result)) return result;
    bool soft_wrapped = false;
    result = ghostty_row_get(raw_row, GHOSTTY_ROW_DATA_WRAP, &soft_wrapped);
    if (!success(result)) return result;
    GhosttyRenderStateRowSelection row_selection =
        GHOSTTY_INIT_SIZED(GhosttyRenderStateRowSelection);
    result = ghostty_render_state_row_get(
        spike->rows, GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
        &row_selection);
    const bool row_selected = success(result);
    if (!row_selected && result != GHOSTTY_NO_VALUE) return result;

    result = ghostty_render_state_row_get(
        spike->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &spike->cells);
    if (!success(result)) return result;

    uint16_t x = 0;
    while (ghostty_render_state_row_cells_next(spike->cells)) {
      if (count < capacity) {
        SpikeCell* cell = &cells[count];
        memset(cell, 0, sizeof(*cell));
        cell->x = x;
        cell->y = y;
        cell->soft_wrapped = soft_wrapped;
        cell->selected = row_selected && x >= row_selection.start_x &&
                         x <= row_selection.end_x;

        GhosttyBuffer text = {
            .ptr = cell->text,
            .cap = sizeof(cell->text),
            .len = 0,
        };
        GhosttyResult text_result = ghostty_render_state_row_cells_get(
            spike->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &text);
        if (success(text_result) && text.len <= sizeof(cell->text)) {
          cell->text_len = (uint8_t)text.len;
        } else {
          static const uint8_t replacement[] = {0xef, 0xbf, 0xbd};
          memcpy(cell->text, replacement, sizeof(replacement));
          cell->text_len = sizeof(replacement);
        }

        GhosttyCell raw = 0;
        GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
        GhosttyResult raw_result = ghostty_render_state_row_cells_get(
            spike->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw);
        if (success(raw_result)) {
          ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide);
        }
        switch (wide) {
          case GHOSTTY_CELL_WIDE_WIDE:
            cell->width = 2;
            break;
          case GHOSTTY_CELL_WIDE_SPACER_TAIL:
            cell->width = 0;
            break;
          default:
            cell->width = 1;
            break;
        }

        GhosttyColorRgb fg = colors.foreground;
        GhosttyColorRgb bg = colors.background;
        GhosttyResult fg_result = ghostty_render_state_row_cells_get(
            spike->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg);
        GhosttyResult bg_result = ghostty_render_state_row_cells_get(
            spike->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg);
        if (!success(fg_result)) fg = colors.foreground;
        if (!success(bg_result)) bg = colors.background;

        GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
        GhosttyResult style_result = ghostty_render_state_row_cells_get(
            spike->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
        bool inverse = success(style_result) && style.inverse;
        if (inverse) {
          GhosttyColorRgb swap = fg;
          fg = bg;
          bg = swap;
        }
        cell->has_explicit_bg = success(bg_result) || inverse;
        set_color(&cell->fg_r, &cell->fg_g, &cell->fg_b, fg);
        set_color(&cell->bg_r, &cell->bg_g, &cell->bg_b, bg);
      }
      count++;
      x++;
    }
    y++;
  }

  if (count > capacity) return GHOSTTY_OUT_OF_MEMORY;

  result = ghostty_render_state_get(spike->render,
                                    GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                                    &spike->rows);
  if (!success(result)) return result;
  while (ghostty_render_state_row_iterator_next(spike->rows)) {
    bool clean = false;
    result = ghostty_render_state_row_set(
        spike->rows, GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY, &clean);
    if (!success(result)) return result;
  }
  GhosttyRenderStateDirty clean = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
  result = ghostty_render_state_set(
      spike->render, GHOSTTY_RENDER_STATE_OPTION_DIRTY, &clean);
  if (!success(result)) return result;

  snapshot->cell_count = count;
  return GHOSTTY_SUCCESS;
}
