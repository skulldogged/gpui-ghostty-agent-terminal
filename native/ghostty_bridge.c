#include "ghostty_bridge.h"

#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>

struct SpikeTerminal {
  GhosttyTerminal terminal;
  GhosttyRenderState render;
  GhosttyRenderStateRowIterator rows;
  GhosttyRenderStateRowCells cells;
};

static bool success(GhosttyResult result) { return result == GHOSTTY_SUCCESS; }

SpikeTerminal* spike_terminal_new(uint16_t cols, uint16_t rows, size_t scrollback) {
  SpikeTerminal* spike = calloc(1, sizeof(SpikeTerminal));
  if (spike == NULL) return NULL;

  GhosttyTerminalOptions options = {
      .cols = cols,
      .rows = rows,
      .max_scrollback = scrollback,
  };
  if (!success(ghostty_terminal_new(NULL, &spike->terminal, options)) ||
      !success(ghostty_render_state_new(NULL, &spike->render)) ||
      !success(ghostty_render_state_row_iterator_new(NULL, &spike->rows)) ||
      !success(ghostty_render_state_row_cells_new(NULL, &spike->cells))) {
    spike_terminal_free(spike);
    return NULL;
  }
  return spike;
}

void spike_terminal_free(SpikeTerminal* spike) {
  if (spike == NULL) return;
  ghostty_render_state_row_cells_free(spike->cells);
  ghostty_render_state_row_iterator_free(spike->rows);
  ghostty_render_state_free(spike->render);
  ghostty_terminal_free(spike->terminal);
  free(spike);
}

void spike_terminal_write(SpikeTerminal* spike, const uint8_t* data, size_t len) {
  if (spike == NULL || data == NULL) return;
  ghostty_terminal_vt_write(spike->terminal, data, len);
}

int spike_terminal_resize(SpikeTerminal* spike, uint16_t cols, uint16_t rows,
                          uint32_t cell_width_px, uint32_t cell_height_px) {
  if (spike == NULL) return GHOSTTY_INVALID_VALUE;
  return ghostty_terminal_resize(spike->terminal, cols, rows, cell_width_px,
                                 cell_height_px);
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
