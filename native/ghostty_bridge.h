#ifndef GHOSTTY_SPIKE_BRIDGE_H
#define GHOSTTY_SPIKE_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SpikeTerminal SpikeTerminal;

typedef struct {
  uint16_t x;
  uint16_t y;
  uint8_t text[32];
  uint8_t text_len;
  uint8_t width;
  uint8_t fg_r;
  uint8_t fg_g;
  uint8_t fg_b;
  uint8_t bg_r;
  uint8_t bg_g;
  uint8_t bg_b;
  bool has_explicit_bg;
  bool soft_wrapped;
  bool selected;
} SpikeCell;

typedef struct {
  uint16_t cols;
  uint16_t rows;
  uint16_t cursor_x;
  uint16_t cursor_y;
  uint8_t title[512];
  uint16_t title_len;
  bool cursor_visible;
  bool bracketed_paste;
  bool alternate_screen;
  bool selection_active;
  uint8_t default_fg_r;
  uint8_t default_fg_g;
  uint8_t default_fg_b;
  uint8_t default_bg_r;
  uint8_t default_bg_g;
  uint8_t default_bg_b;
  bool full;
  size_t cell_count;
} SpikeSnapshot;

typedef struct {
  uint8_t type;
  uint8_t click_count;
  uint16_t x;
  uint16_t y;
  float pointer_x;
  float pointer_y;
  uint32_t columns;
  uint32_t cell_width;
  uint32_t padding_left;
  uint32_t screen_height;
} SpikeSelectionInput;

SpikeTerminal* spike_terminal_new(uint16_t cols, uint16_t rows, size_t scrollback);
void spike_terminal_free(SpikeTerminal* terminal);
void spike_terminal_write(SpikeTerminal* terminal, const uint8_t* data, size_t len);
int spike_terminal_resize(SpikeTerminal* terminal, uint16_t cols, uint16_t rows,
                          uint32_t cell_width_px, uint32_t cell_height_px);
int spike_terminal_set_theme(SpikeTerminal* terminal, const uint8_t* foreground,
                             const uint8_t* background, const uint8_t* cursor,
                             const uint8_t* ansi_palette);
int spike_terminal_encode_paste(SpikeTerminal* terminal, uint8_t* data,
                                 size_t data_len, uint8_t* output,
                                 size_t output_len, size_t* output_written);
int spike_terminal_scroll(SpikeTerminal* terminal, intptr_t delta_rows,
                          intptr_t delta_columns,
                          float pointer_x, float pointer_y,
                          uint32_t viewport_width, uint32_t viewport_height,
                          uint32_t cell_width, uint32_t cell_height,
                          uint16_t modifiers, uint8_t* output,
                          size_t output_len, size_t* output_written,
                          bool* viewport_changed);
int spike_terminal_scroll_to_bottom(SpikeTerminal* terminal, bool* changed);
int spike_terminal_selection_event(SpikeTerminal* terminal,
                                   const SpikeSelectionInput* input,
                                   bool* viewport_changed);
int spike_terminal_selection_text(SpikeTerminal* terminal, uint8_t* output,
                                  size_t output_len, size_t* output_written);
int spike_terminal_snapshot(SpikeTerminal* terminal, bool force_full,
                             SpikeSnapshot* snapshot, SpikeCell* cells,
                             size_t capacity);

#ifdef __cplusplus
}
#endif

#endif
