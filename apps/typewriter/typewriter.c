/*
 * Typewriter (Машинка) — a quiet place to write on a typewriter page.
 * An A4 sheet in the TT2020 typeface, tabs, a file tree, a terminal,
 * PDF export, live tables. Written by Roman Kolida; part of Chlorum.
 * The interface is in English like the rest of Chlorum; comments in the
 * code stay in Ukrainian.
 *
 * Build:
 *   gcc -O2 typewriter.c -o typewriter \
 *     $(pkg-config --cflags --libs sdl2 freetype2 harfbuzz fontconfig) -lz -lm -lutil
 *
 * Run:  typewriter [folder or files...]
 * The TT2020 fonts live in fonts/ next to the program, or in
 * ../share/mashynka/fonts.
 */
#define _GNU_SOURCE
#include <SDL.h>
#include <ft2build.h>
#include FT_FREETYPE_H
#include <hb.h>
#include <fontconfig/fontconfig.h>
#include <zlib.h>
#include <dirent.h>
#include <errno.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <termios.h>
#include <fcntl.h>
#include <time.h>
#include <unistd.h>
#ifdef __APPLE__
#include <util.h>
#else
#include <pty.h>
#endif
#include <math.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>

#define MM (72.0 / 25.4)
#define PAGE_W (210 * MM)
#define PAGE_H (297 * MM)
#define GAP_PT 12.0
#define TAB_STOP 4
#define UNDO_MEM (256u << 20)
#define MAXDOCS 64
#define ID (__LINE__ * 256)

/* фіксовані розміри панелей (логічні пікселі) */
#define SIDE_W 240
#define TERM_H 230
#define TABS_H 38
#define PAGE_PAD 16
#define RIGHT_W 300

#ifdef __APPLE__
#define CMDMOD KMOD_GUI
#else
#define CMDMOD KMOD_CTRL
#endif

/* ================= налаштування =================
 * ~/.config/mashynka.conf, значення — номери варіантів:
 *   style   0 E, 1 B, 2 D           (стиль TT2020, також Ctrl+T)
 *   size    0 10pt, 1 11pt, 2 12pt, 3 14pt
 *   spacing 0 1.0, 1 1.15, 2 1.5, 3 2.0
 *   margins 0 5мм, 1 10мм, 2 15мм, 3 20мм  (поля аркуша)
 *   ink     0 чорне, 1 синє, 2 червоне, 3 сепія
 *   paper   0 білий, 1 кремовий, 2 старий
 *   ui      0 авто, 1 100%, 2 125%, 3 150%, 4 200%
 *   zoom    масштаб аркуша у %, 0 — на всю ширину (Ctrl+= / Ctrl+- / Ctrl+0)
 *   calib   калібрування справжнього розміру у % (Ctrl+Alt+= / Ctrl+Alt+-)
 */
typedef struct { Uint8 r, g, b; } RGB;

static const char *STYLE_FILE[] = {"TT2020StyleE-Regular", "TT2020StyleB-Regular", "TT2020StyleD-Regular"};
static const char *STYLE_LBL[] = {"E", "B", "D"};
static const double SIZES[] = {10, 11, 12, 14};
static const double SPACING[] = {1.0, 1.15, 1.5, 2.0};
static const double MARGINS[] = {5, 10, 15, 20};
static const RGB INKS[] = {{28, 27, 25}, {30, 52, 120}, {155, 32, 28}, {96, 66, 40}};
static const RGB PAPERS[] = {{255, 255, 255}, {252, 250, 243}, {243, 234, 212}};
static const double UIS[] = {0, 1.0, 1.25, 1.5, 2.0};

typedef struct {
  int style, size, spacing, margin, ink, paper, ui, zoom, calib;
  int hidden;    /* показувати приховані файли (.bashrc, .config…) у провіднику */
  int pdfmode;   /* PDF: 0 як на екрані (колір паперу й чорнила), 1 для друку — чорне на білому */
  int pagenum;   /* номер сторінки внизу праворуч: 0 вимк, 1 лише на екрані, 2 на екрані й у PDF */
  int header;    /* шапка «page N lines a-b»: 0 вимк, 1 лише на екрані, 2 на екрані й у PDF */
  int tz;        /* часовий пояс зі списку TZS */
  int datefmt;   /* формат дати для швидкої вставки */
  char author[128], tzcustom[64];
  char dir[1024];
} Settings;
static Settings S = {.size = 2, .paper = 1, .zoom = 100, .calib = 100, .hidden = 1};
static int HDR, FTR, TPP = 40; /* рядок шапки, рядок номера, текстових рядків на сторінці */
/* масштаб аркуша у відсотках від справжнього розміру; 0 — на всю ширину */
static const int ZOOMS[] = {50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300};
#define NZOOMS ((int)(sizeof ZOOMS / sizeof ZOOMS[0]))

/* ================= шорткати =================
 * Кожна дія має до трьох комбінацій. Клавіші розпізнаються за фізичним положенням
 * (scancode), тож працюють і в українській розкладці. */
enum { MD_CTRL = 1, MD_SHIFT = 2, MD_ALT = 4, MD_SUPER = 8 };
typedef struct { int sc; int mod; } Bind;
enum { CX_GLOBAL, CX_EDIT, CX_FILES };
typedef struct { const char *id, *label; int cx; const char *def[3]; Bind b[3]; } Action;
enum {
  A_NEW, A_OPEN, A_SAVE, A_SAVE_AS, A_PDF, A_CLOSE, A_QUIT, A_SETTINGS, A_INSERT, A_FIND, A_REPLACE,
  A_FIND_NEXT, A_FIND_PREV, A_TAB_NEXT, A_TAB_PREV, A_TERM, A_STYLE, A_VIEW, A_ZOOM_IN, A_ZOOM_OUT,
  A_ZOOM_RESET, A_CALIB_UP, A_CALIB_DOWN,
  A_UNDO, A_REDO, A_ALL, A_COPY, A_CUT, A_PASTE, A_DUP, A_DEL_LINE, A_MOVE_UP, A_MOVE_DOWN, A_SEL_LINE,
  A_INDENT, A_OUTDENT, A_DATE, A_TABLE_FMT, A_CENTER, A_TITLE,
  A_TRASH, A_TRASH_UNDO, A_TREE_UP, A_SLIDES,
  A_ROW_BELOW, A_ROW_ABOVE, A_COL_RIGHT, A_COL_LEFT, A_ROW_DEL, A_COL_DEL, A_COL_WIDER, A_COL_NARROWER, NACT
};
static Action ACTS[NACT] = {
    [A_NEW] = {"new", "New tab", CX_GLOBAL, {"Ctrl+N"}},
    [A_OPEN] = {"open", "Open file", CX_GLOBAL, {"Ctrl+O"}},
    [A_SAVE] = {"save", "Save", CX_GLOBAL, {"Ctrl+S"}},
    [A_SAVE_AS] = {"save_as", "Save as", CX_GLOBAL, {"Ctrl+Shift+S"}},
    [A_PDF] = {"pdf", "Export to PDF", CX_GLOBAL, {"Ctrl+E", "Ctrl+P"}},
    [A_CLOSE] = {"close", "Close tab", CX_GLOBAL, {"Ctrl+W"}},
    [A_QUIT] = {"quit", "Quit", CX_GLOBAL, {"Ctrl+Q"}},
    [A_SETTINGS] = {"settings", "Settings", CX_GLOBAL, {"Ctrl+,"}},
    [A_INSERT] = {"insert", "Insert and format", CX_GLOBAL, {"Ctrl+I"}},
    [A_FIND] = {"find", "Find", CX_GLOBAL, {"Ctrl+F"}},
    [A_REPLACE] = {"replace", "Find and replace", CX_GLOBAL, {"Ctrl+H"}},
    [A_FIND_NEXT] = {"find_next", "Next match", CX_GLOBAL, {"F3"}},
    [A_FIND_PREV] = {"find_prev", "Previous match", CX_GLOBAL, {"Shift+F3"}},
    [A_TAB_NEXT] = {"tab_next", "Next tab", CX_GLOBAL, {"Ctrl+Tab", "Ctrl+PageDown"}},
    [A_TAB_PREV] = {"tab_prev", "Previous tab", CX_GLOBAL, {"Ctrl+Shift+Tab", "Ctrl+PageUp"}},
    [A_TERM] = {"terminal", "Terminal ↔ text", CX_GLOBAL, {"Ctrl+`"}},
    [A_STYLE] = {"style", "TT2020 ribbon style", CX_GLOBAL, {"Ctrl+T"}},
    [A_VIEW] = {"view", "Machine code: asm ↔ hex", CX_GLOBAL, {"Ctrl+B"}},
    [A_ZOOM_IN] = {"zoom_in", "Zoom in", CX_GLOBAL, {"Ctrl+=", "Ctrl+Shift+=", "Ctrl+Keypad +"}},
    [A_ZOOM_OUT] = {"zoom_out", "Zoom out", CX_GLOBAL, {"Ctrl+-", "Ctrl+Keypad -"}},
    [A_ZOOM_RESET] = {"zoom_reset", "100% ↔ fit width", CX_GLOBAL, {"Ctrl+0"}},
    [A_CALIB_UP] = {"calib_up", "Calibration +", CX_GLOBAL, {"Ctrl+Alt+="}},
    [A_CALIB_DOWN] = {"calib_down", "Calibration −", CX_GLOBAL, {"Ctrl+Alt+-"}},
    [A_UNDO] = {"undo", "Undo", CX_EDIT, {"Ctrl+Z"}},
    [A_REDO] = {"redo", "Redo", CX_EDIT, {"Ctrl+Y", "Ctrl+Shift+Z"}},
    [A_ALL] = {"select_all", "Select all", CX_EDIT, {"Ctrl+A"}},
    [A_COPY] = {"copy", "Copy", CX_EDIT, {"Ctrl+C"}},
    [A_CUT] = {"cut", "Cut", CX_EDIT, {"Ctrl+X"}},
    [A_PASTE] = {"paste", "Paste", CX_EDIT, {"Ctrl+V"}},
    [A_DUP] = {"dup_line", "Duplicate line", CX_EDIT, {"Ctrl+D"}},
    [A_DEL_LINE] = {"del_line", "Delete line", CX_EDIT, {"Ctrl+Shift+K"}},
    [A_MOVE_UP] = {"move_up", "Move line up", CX_EDIT, {"Alt+Up"}},
    [A_MOVE_DOWN] = {"move_down", "Move line down", CX_EDIT, {"Alt+Down"}},
    [A_SEL_LINE] = {"select_line", "Select line", CX_EDIT, {"Ctrl+L"}},
    [A_INDENT] = {"indent", "Indent", CX_EDIT, {"Ctrl+]"}},
    [A_OUTDENT] = {"outdent", "Outdent", CX_EDIT, {"Ctrl+["}},
    [A_DATE] = {"date", "Insert date", CX_EDIT, {"Ctrl+Shift+D"}},
    [A_TABLE_FMT] = {"table", "Align table", CX_EDIT, {"Ctrl+Alt+T"}},
    [A_CENTER] = {"center", "Center line", CX_EDIT, {"Ctrl+Alt+C"}},
    [A_TITLE] = {"title", "Make a title", CX_EDIT, {"Ctrl+Alt+1"}},
    [A_TRASH] = {"trash", "Move file to the bin", CX_FILES, {"Delete"}},
    [A_TRASH_UNDO] = {"trash_undo", "Restore from the bin", CX_FILES, {"Ctrl+Z"}},
    [A_TREE_UP] = {"tree_up", "Folder up", CX_FILES, {"Backspace"}},
    [A_SLIDES] = {"slides", "Slideshow of the folder", CX_GLOBAL, {"F5"}},
    [A_ROW_BELOW] = {"row_below", "Table: row below", CX_EDIT, {"Ctrl+Alt+Down"}},
    [A_ROW_ABOVE] = {"row_above", "Table: row above", CX_EDIT, {"Ctrl+Alt+Up"}},
    [A_COL_RIGHT] = {"col_right", "Table: column right", CX_EDIT, {"Ctrl+Alt+Right"}},
    [A_COL_LEFT] = {"col_left", "Table: column left", CX_EDIT, {"Ctrl+Alt+Left"}},
    [A_ROW_DEL] = {"row_del", "Table: delete row", CX_EDIT, {"Ctrl+Alt+Backspace"}},
    [A_COL_DEL] = {"col_del", "Table: delete column", CX_EDIT, {"Ctrl+Alt+Shift+Backspace"}},
    [A_COL_WIDER] = {"col_wider", "Table: wider column", CX_EDIT, {"Alt+Shift+Right"}},
    [A_COL_NARROWER] = {"col_narrower", "Table: narrower column", CX_EDIT, {"Alt+Shift+Left"}},
};

static int mods_of(Uint16 m) {
  return ((m & KMOD_CTRL) ? MD_CTRL : 0) | ((m & KMOD_SHIFT) ? MD_SHIFT : 0) | ((m & KMOD_ALT) ? MD_ALT : 0) |
         ((m & KMOD_GUI) ? MD_SUPER : 0);
}

static void bind_str(Bind b, char *out, size_t n) {
  if (!b.sc) { snprintf(out, n, "—"); return; }
  snprintf(out, n, "%s%s%s%s%s", (b.mod & MD_CTRL) ? "Ctrl+" : "", (b.mod & MD_SUPER) ? "Super+" : "",
           (b.mod & MD_ALT) ? "Alt+" : "", (b.mod & MD_SHIFT) ? "Shift+" : "", SDL_GetScancodeName((SDL_Scancode)b.sc));
}

static Bind bind_parse(const char *s) {
  Bind b = {0, 0};
  for (;;) {
    if (!strncmp(s, "Ctrl+", 5)) { b.mod |= MD_CTRL; s += 5; }
    else if (!strncmp(s, "Shift+", 6)) { b.mod |= MD_SHIFT; s += 6; }
    else if (!strncmp(s, "Alt+", 4)) { b.mod |= MD_ALT; s += 4; }
    else if (!strncmp(s, "Super+", 6)) { b.mod |= MD_SUPER; s += 6; }
    else break;
  }
  if (!*s) return (Bind){0, 0};
  b.sc = SDL_GetScancodeFromName(s);
  if (b.sc == SDL_SCANCODE_UNKNOWN) b.mod = 0;
  return b;
}

static void keys_default(int a) { for (int k = 0; k < 3; k++) ACTS[a].b[k] = ACTS[a].def[k] ? bind_parse(ACTS[a].def[k]) : (Bind){0, 0}; }
static void keys_init(void) { for (int a = 0; a < NACT; a++) keys_default(a); }

static int hit(int a, SDL_Keysym ks) {
  int m = mods_of(ks.mod);
  for (int k = 0; k < 3; k++)
    if (ACTS[a].b[k].sc && ACTS[a].b[k].sc == (int)ks.scancode && ACTS[a].b[k].mod == m) return 1;
  return 0;
}

/* Чи ця комбінація вже зайнята іншою дією в тому самому місці. */
static int bind_conflict(int a, int k) {
  Bind b = ACTS[a].b[k];
  if (!b.sc) return 0;
  for (int i = 0; i < NACT; i++)
    for (int j = 0; j < 3; j++) {
      if (i == a && j == k) continue;
      if (ACTS[i].b[j].sc == b.sc && ACTS[i].b[j].mod == b.mod &&
          (ACTS[i].cx == ACTS[a].cx || ACTS[i].cx == CX_GLOBAL || ACTS[a].cx == CX_GLOBAL)) return 1;
    }
  return 0;
}

/* тема: усе темне, світлий лише аркуш */
static const RGB C_DESK = {23, 22, 20}, C_PANEL = {30, 29, 27}, C_PANEL2 = {38, 37, 34}, C_HOVER = {44, 42, 39},
                 C_LINE = {46, 44, 41}, C_TXT = {218, 213, 203}, C_DIM = {140, 135, 126}, C_FAINT = {96, 92, 86},
                 C_ACC = {226, 176, 104}, C_ACC_DARK = {40, 30, 18}, C_FIELD = {24, 23, 21}, C_SEL = {255, 208, 120},
                 C_CARET = {176, 42, 32};

/* ================= загальний стан ================= */
static SDL_Window *W;
static SDL_Renderer *R;
static int ow, oh, running = 1;
static double pr = 1, u = 1;
static FT_Library ftl;
static hb_buffer_t *hbbuf;
static char root[1024];

static double FONT_PT = 12, LEAD_PT = 14, MARGIN_X, MARGIN_TOP, MARGIN_BOT, ADVPT, BASEPT;
static int COLS = 60, LPP = 40, g_lgen = 1;

/* ================= документ ================= */
typedef struct { int start, len, src; } Line; /* src — номер рядка у файлі */
typedef struct { uint32_t *c; int n, cur; } Snap;
typedef struct {
  uint32_t *c;
  int n, cap;
  Line *L;
  int nL, capL, lgen;
  int cur, anchor, desired_col, dirty;
  double scroll;
  char path[2048];
  Snap *us, *rs;
  int nus, nrs, cus, crs;
  size_t umem;
  int last_kind, last_cur;
  Uint32 last_t;
  unsigned ver;
  int ro, view;        /* лише читання; вигляд (текст, hex, asm, pdf, картинка, слайдшоу…) */
  int ftype, enc, crlf, bom; /* тип файлу за вмістом, кодування, закінчення рядків CRLF, BOM */
  struct Media_ *media; /* картинка / слайдшоу / сторінки PDF */
  unsigned uid;        /* постійний номер вкладки для збереження сеансу */
  unsigned buf_ver;    /* версія, яку востаннє записано в чернетку сеансу */
  int buf_saved;
} Doc;
enum { VIEW_TEXT, VIEW_HEX, VIEW_ASM, VIEW_EXTRACT, VIEW_PDF, VIEW_IMAGE, VIEW_SLIDES };
static int is_media_view(int v) { return v == VIEW_IMAGE || v == VIEW_SLIDES || v == VIEW_PDF; }
enum { FT_TEXT, FT_PDF, FT_IMAGE, FT_ARCHIVE, FT_DOCX, FT_ODT, FT_EPUB, FT_MEDIA, FT_ELF, FT_BINARY };
enum { ENC_UTF8, ENC_CP1251, ENC_UTF16LE, ENC_UTF16BE };
static const char *ENC_NAME[] = {"UTF-8", "Windows-1251", "UTF-16 LE", "UTF-16 BE"};
struct Media_;
static void media_free(struct Media_ *m);
static int media_key(SDL_Keysym ks);
static unsigned uid_next;
static Doc *docs[MAXDOCS];
static int ndocs, act;
static Doc *D;
static int need_visible = 1;

/* ================= UTF-8 ================= */
static int utf8_enc(uint32_t c, char *o) {
  if (c < 0x80) { o[0] = (char)c; return 1; }
  if (c < 0x800) { o[0] = 0xC0 | (c >> 6); o[1] = 0x80 | (c & 63); return 2; }
  if (c < 0x10000) { o[0] = 0xE0 | (c >> 12); o[1] = 0x80 | ((c >> 6) & 63); o[2] = 0x80 | (c & 63); return 3; }
  o[0] = 0xF0 | (c >> 18); o[1] = 0x80 | ((c >> 12) & 63); o[2] = 0x80 | ((c >> 6) & 63); o[3] = 0x80 | (c & 63);
  return 4;
}

/* Розбирає UTF-8 у кодові точки: \r\n -> \n, табуляція -> пробіли, керівні символи відкидаються. */
static uint32_t *utf8_dec(const char *s, size_t n, int *outn) {
  uint32_t *o = malloc((n * 4 + 4) * sizeof(uint32_t));
  int k = 0;
  const unsigned char *b = (const unsigned char *)s;
  for (size_t i = 0; i < n;) {
    uint32_t c = b[i];
    int len = 1;
    if (c >= 0xF0 && i + 3 < n) { c = ((c & 7) << 18) | ((b[i+1] & 63) << 12) | ((b[i+2] & 63) << 6) | (b[i+3] & 63); len = 4; }
    else if (c >= 0xE0 && i + 2 < n) { c = ((c & 15) << 12) | ((b[i+1] & 63) << 6) | (b[i+2] & 63); len = 3; }
    else if (c >= 0xC0 && i + 1 < n) { c = ((c & 31) << 6) | (b[i+1] & 63); len = 2; }
    else if (c >= 0x80) c = 0xFFFD;
    i += len;
    if (c == '\r') { if (i < n && b[i] == '\n') continue; c = '\n'; }
    if (c == '\t') { for (int t = 0; t < TAB_STOP; t++) o[k++] = ' '; continue; }
    if ((c < 32 && c != '\n') || c == 0xFEFF) continue;
    o[k++] = c;
  }
  *outn = k;
  return o;
}

static char *text_utf8(const uint32_t *c, int a, int b, size_t *outlen) {
  char *o = malloc((size_t)(b - a) * 4 + 1);
  size_t k = 0;
  for (int i = a; i < b; i++) k += utf8_enc(c[i], o + k);
  o[k] = 0;
  if (outlen) *outlen = k;
  return o;
}

/* ================= розкладка ================= */
static int layout_src;
static void addline(Doc *d, int s, int len) {
  if (d->nL == d->capL) { d->capL = d->capL ? d->capL * 2 : 256; d->L = realloc(d->L, d->capL * sizeof(Line)); }
  d->L[d->nL].start = s;
  d->L[d->nL].len = len;
  d->L[d->nL].src = layout_src;
  d->nL++;
}

static void layout(Doc *d) {
  d->nL = 0;
  layout_src = 0;
  int i = 0;
  for (;;) {
    int pe = i;
    while (pe < d->n && d->c[pe] != '\n') pe++;
    int s = i;
    if (s == pe) addline(d, s, 0);
    while (s < pe) {
      int rem = pe - s;
      if (rem <= COLS) { addline(d, s, rem); break; }
      int b = -1;
      for (int k = s + COLS; k > s; k--)
        if (d->c[k] == ' ') { b = k; break; }
      if (b < 0) { addline(d, s, COLS); s += COLS; }
      else { addline(d, s, b - s + 1); s = b + 1; }
    }
    if (pe >= d->n) break;
    i = pe + 1;
    layout_src++;
  }
  d->lgen = g_lgen;
}

static void ensure_layout(Doc *d) { if (d->lgen != g_lgen) layout(d); }

static int line_of(int pos) {
  int lo = 0, hi = D->nL - 1;
  while (lo < hi) {
    int m = (lo + hi + 1) / 2;
    if (D->L[m].start <= pos) lo = m; else hi = m - 1;
  }
  return lo;
}

static int maxcol(int l) {
  int wrapped = l + 1 < D->nL && D->L[l + 1].start == D->L[l].start + D->L[l].len;
  return wrapped ? D->L[l].len - 1 : D->L[l].len;
}

static int npages(void) { int p = (D->nL + TPP - 1) / TPP; return p < 1 ? 1 : p; }

/* ================= правка й скасування ================= */
static void reserve(Doc *d, int n) {
  if (n + 1 > d->cap) { d->cap = (n + 1) + n / 2 + 64; d->c = realloc(d->c, d->cap * sizeof(uint32_t)); }
}

static Snap snap_now(void) {
  Snap s = {malloc((D->n + 1) * sizeof(uint32_t)), D->n, D->cur};
  memcpy(s.c, D->c, D->n * sizeof(uint32_t));
  return s;
}

static void snap_push(Snap **st, int *n, int *cap, Snap s) {
  if (*n == *cap) { *cap = *cap ? *cap * 2 : 64; *st = realloc(*st, *cap * sizeof(Snap)); }
  (*st)[(*n)++] = s;
}

static void clear_redo(Doc *d) { while (d->nrs) free(d->rs[--d->nrs].c); }

static void clear_history(Doc *d) {
  while (d->nus) free(d->us[--d->nus].c);
  clear_redo(d);
  d->umem = 0;
  d->last_kind = 0;
}

static void push_undo(Snap s) {
  snap_push(&D->us, &D->nus, &D->cus, s);
  D->umem += (size_t)s.n * 4;
  while (D->umem > UNDO_MEM && D->nus > 1) {
    D->umem -= (size_t)D->us[0].n * 4;
    free(D->us[0].c);
    memmove(D->us, D->us + 1, (D->nus - 1) * sizeof(Snap));
    D->nus--;
  }
}

static void set_toast(const char *msg);
/* Машинний код відкривається лише для читання. */
static int ro_block(void) {
  if (!D->ro) return 0;
  set_toast("Machine code is open read-only");
  return 1;
}

/* kind: 1 друк, 2 видалення, 0 інше (завжди нова точка скасування) */
static void before_edit(int kind) {
  Uint32 now = SDL_GetTicks();
  int same = kind && kind == D->last_kind && D->cur == D->last_cur && D->anchor == D->cur && now - D->last_t < 1500;
  if (!same) { push_undo(snap_now()); clear_redo(D); }
  D->last_kind = kind;
  D->last_t = now;
}

static void after_edit(void) {
  D->last_cur = D->cur;
  D->anchor = D->cur;
  D->dirty = 1;
  D->desired_col = -1;
  D->ver++;
  need_visible = 1;
  layout(D);
}

static void restore(Snap s) {
  reserve(D, s.n);
  memcpy(D->c, s.c, s.n * sizeof(uint32_t));
  D->n = s.n;
  D->cur = D->anchor = s.cur > D->n ? D->n : s.cur;
  free(s.c);
  D->last_kind = 0; D->dirty = 1; D->ver++;
  need_visible = 1;
  layout(D);
}

static void undo(void) {
  if (ro_block()) return;
  if (!D->nus) return;
  snap_push(&D->rs, &D->nrs, &D->crs, snap_now());
  Snap s = D->us[--D->nus];
  D->umem -= (size_t)s.n * 4;
  restore(s);
}

static void redo(void) {
  if (ro_block()) return;
  if (!D->nrs) return;
  push_undo(snap_now());
  restore(D->rs[--D->nrs]);
}

static int sel_a(void) { return D->cur < D->anchor ? D->cur : D->anchor; }
static int sel_b(void) { return D->cur < D->anchor ? D->anchor : D->cur; }

/* Замінює [a, b) на s без запису в історію. */
static void replace_range(int a, int b, const uint32_t *s, int n) {
  int delta = n - (b - a);
  reserve(D, D->n + delta);
  memmove(D->c + a + n, D->c + b, (size_t)(D->n - b) * sizeof(uint32_t));
  if (n) memcpy(D->c + a, s, (size_t)n * sizeof(uint32_t));
  D->n += delta;
}

static void insert(const uint32_t *s, int n, int kind) {
  if (ro_block()) return;
  before_edit(kind);
  int a = sel_a(), b = sel_b();
  replace_range(a, b, s, n);
  D->cur = a + n;
  after_edit();
}

static int is_word(uint32_t c) { return c != ' ' && c != '\n'; }
static int word_left(int p) {
  while (p > 0 && !is_word(D->c[p - 1])) p--;
  while (p > 0 && is_word(D->c[p - 1])) p--;
  return p;
}
static int word_right(int p) {
  while (p < D->n && !is_word(D->c[p])) p++;
  while (p < D->n && is_word(D->c[p])) p++;
  return p;
}

static void erase(int forward, int word) {
  if (ro_block()) return;
  int a, b;
  if (D->cur != D->anchor) { a = sel_a(); b = sel_b(); }
  else if (forward) { a = D->cur; b = word ? word_right(D->cur) : (D->cur < D->n ? D->cur + 1 : D->cur); }
  else { b = D->cur; a = word ? word_left(D->cur) : (D->cur > 0 ? D->cur - 1 : 0); }
  if (a == b) return;
  before_edit(word || D->cur != D->anchor ? 0 : 2);
  replace_range(a, b, NULL, 0);
  D->cur = a;
  after_edit();
}

/* ================= операції над рядками (абзацами) ================= */
static int lstart(int p) { while (p > 0 && D->c[p - 1] != '\n') p--; return p; }
static int lend(int p) { while (p < D->n && D->c[p] != '\n') p++; return p; }

/* Межі цілих рядків, які зачіпає виділення: [a, b) без кінцевого \n. */
static void sel_lines(int *a, int *b) {
  int s = sel_a(), e = sel_b();
  if (e > s && D->c[e - 1] == '\n') e--;
  *a = lstart(s);
  *b = lend(e);
}

static void lines_delete(void) {
  if (ro_block()) return;
  int a, b;
  sel_lines(&a, &b);
  before_edit(0);
  if (b < D->n) b++;
  else if (a > 0) a--;
  replace_range(a, b, NULL, 0);
  D->cur = a < D->n ? lstart(a) : a;
  after_edit();
}

static void lines_duplicate(void) {
  if (ro_block()) return;
  int a, b, oc = D->cur, oa = D->anchor;
  sel_lines(&a, &b);
  int n = b - a;
  uint32_t *t = malloc((size_t)(n + 1) * sizeof(uint32_t));
  t[0] = '\n';
  memcpy(t + 1, D->c + a, (size_t)n * sizeof(uint32_t));
  before_edit(0);
  replace_range(b, b, t, n + 1);
  free(t);
  after_edit();
  D->cur = oc + n + 1;
  D->anchor = oa + n + 1;
}

static void lines_move(int dir) {
  if (ro_block()) return;
  int a, b;
  sel_lines(&a, &b);
  int s = sel_a(), e = sel_b(), cur_first = D->cur < D->anchor;
  if (dir < 0 && a == 0) return;
  if (dir > 0 && b >= D->n) return;
  int n = b - a;
  uint32_t *blk = malloc((size_t)(n + 1) * sizeof(uint32_t));
  memcpy(blk, D->c + a, (size_t)n * sizeof(uint32_t));
  before_edit(0);
  int shift;
  if (dir < 0) {
    int pa = lstart(a - 1), pn = a - 1 - pa;
    uint32_t *prev = malloc((size_t)(pn + 1) * sizeof(uint32_t));
    memcpy(prev, D->c + pa, (size_t)pn * sizeof(uint32_t));
    uint32_t *nw = malloc((size_t)(n + pn + 1) * sizeof(uint32_t));
    memcpy(nw, blk, (size_t)n * sizeof(uint32_t));
    nw[n] = '\n';
    memcpy(nw + n + 1, prev, (size_t)pn * sizeof(uint32_t));
    replace_range(pa, b, nw, n + 1 + pn);
    free(prev); free(nw);
    shift = -(pn + 1);
  } else {
    int nb = lend(b + 1), nn = nb - (b + 1);
    uint32_t *nxt = malloc((size_t)(nn + 1) * sizeof(uint32_t));
    memcpy(nxt, D->c + b + 1, (size_t)nn * sizeof(uint32_t));
    uint32_t *nw = malloc((size_t)(n + nn + 1) * sizeof(uint32_t));
    memcpy(nw, nxt, (size_t)nn * sizeof(uint32_t));
    nw[nn] = '\n';
    memcpy(nw + nn + 1, blk, (size_t)n * sizeof(uint32_t));
    replace_range(a, nb, nw, n + 1 + nn);
    free(nxt); free(nw);
    shift = nn + 1;
  }
  free(blk);
  after_edit();
  if (cur_first) { D->cur = s + shift; D->anchor = e + shift; }
  else { D->anchor = s + shift; D->cur = e + shift; }
}

/* Відступ (dir > 0) або зняття відступу (dir < 0) для всіх рядків виділення. */
static void lines_indent(int dir) {
  if (ro_block()) return;
  int a, b, had = D->cur != D->anchor, s = sel_a();
  sel_lines(&a, &b);
  int cap = (b - a) * 2 + TAB_STOP * 4 + 64, k = 0, first_delta = 0, first = 1;
  uint32_t *o = malloc((size_t)cap * sizeof(uint32_t));
  for (int i = a;;) {
    int le = i;
    while (le < b && D->c[le] != '\n') le++;
    int need = k + TAB_STOP + (le - i) + 2;
    if (need > cap) { cap = need * 2; o = realloc(o, (size_t)cap * sizeof(uint32_t)); }
    int delta = 0, from = i;
    if (dir > 0) { for (int t = 0; t < TAB_STOP; t++) o[k++] = ' '; delta = TAB_STOP; }
    else { while (from - i < TAB_STOP && from < le && D->c[from] == ' ') from++; delta = -(from - i); }
    if (first) { first_delta = delta; first = 0; }
    memcpy(o + k, D->c + from, (size_t)(le - from) * sizeof(uint32_t));
    k += le - from;
    if (le >= b) break;
    o[k++] = '\n';
    i = le + 1;
  }
  before_edit(0);
  replace_range(a, b, o, k);
  free(o);
  after_edit();
  if (had) { D->anchor = a; D->cur = a + k; }
  else { int p = s + first_delta; if (p < a) p = a; D->cur = D->anchor = p; }
}

static void select_lines(void) {
  int a, b;
  sel_lines(&a, &b);
  D->anchor = a;
  D->cur = b < D->n ? b + 1 : b;
  need_visible = 1;
}

/* ================= форматування простим текстом =================
 * Усе — лише символи: заголовок — великі літери по центру з лінією «=====»,
 * таблиця — з «+», «-» і «|». Жодного жирного, курсиву чи графіки. */
typedef struct { uint32_t *p; int n, cap; } U32;
static void u_put(U32 *b, uint32_t c) {
  if (b->n == b->cap) { b->cap = b->cap ? b->cap * 2 : 256; b->p = realloc(b->p, (size_t)b->cap * sizeof(uint32_t)); }
  b->p[b->n++] = c;
}
static void u_rep(U32 *b, uint32_t c, int k) { while (k-- > 0) u_put(b, c); }
static void u_arr(U32 *b, const uint32_t *a, int n) { for (int i = 0; i < n; i++) u_put(b, a[i]); }
static void u_str(U32 *b, const char *s) { int n; uint32_t *c = utf8_dec(s, strlen(s), &n); u_arr(b, c, n); free(c); }


/* Межі абзаців, яких торкається виділення (без кінцевого \n). */
static void para_range(int *a, int *b) {
  int s0 = D->cur < D->anchor ? D->cur : D->anchor, e0 = D->cur < D->anchor ? D->anchor : D->cur;
  if (e0 > s0 && D->c[e0 - 1] == '\n') e0--;
  while (s0 > 0 && D->c[s0 - 1] != '\n') s0--;
  while (e0 < D->n && D->c[e0] != '\n') e0++;
  *a = s0; *b = e0;
}

/* Замінює [a, b) новим текстом як один крок скасування; курсор ставить на a + cur_off. */
static void replace_block(int a, int b, U32 *t, int cur_off) {
  before_edit(0);
  replace_range(a, b, t->p, t->n);
  D->cur = a + (cur_off < 0 ? t->n : cur_off);
  after_edit();
  free(t->p);
}

static int is_rule_line(const uint32_t *c, int n) { /* рядок лише з «=», «-» і пробілів */
  int k = 0;
  for (int i = 0; i < n; i++) { if (c[i] == '=' || c[i] == '-') k++; else if (c[i] != ' ') return 0; }
  return k > 0;
}

enum { FMT_TITLE, FMT_SUBTITLE, FMT_SECTION, FMT_CENTER, FMT_RIGHT, FMT_LEFT, FMT_NUMBERS, FMT_BULLETS };

/* Перетворює рядки виділення: заголовок, вирівнювання, списки. */
static int list_item(int ls, int le, int *ind, int *num, int *mlen);
static int tbl_block(int p, int *a, int *b, int *indent);
enum { FMT_UNLIST = 100 };

/* Повторна команда знімає форматування: заголовок стає звичайним рядком,
   центрований текст — ліворуч, список — звичайними рядками. */
static void format_lines(int kind) {
  if (ro_block()) return;
  { int ta, tb, ti; if (tbl_block(D->cur, &ta, &tb, &ti)) { set_toast("Line formatting does not apply inside a table"); return; } }
  int a, b;
  para_range(&a, &b);
  /* що вже є: вирівнювання, список, лінія під текстом */
  int all_c = 1, all_r = 1, all_n = 1, all_b = 1, any = 0;
  for (int i = a; i <= b;) {
    int e = lend(i), ts = i, te = e;
    while (ts < te && D->c[ts] == ' ') ts++;
    while (te > ts && D->c[te - 1] == ' ') te--;
    int len = te - ts, lead = ts - i, li, num, ml, lk;
    if (len) {
      any = 1;
      if (lead < 1 || abs(lead - (COLS - len) / 2) > 1) all_c = 0;
      if (lead < 1 || te - i < COLS - 1) all_r = 0;
      lk = list_item(i, e, &li, &num, &ml);
      if (lk != 1) all_n = 0;
      if (lk != 2 && lk != 3) all_b = 0;
    }
    if (e >= b) break;
    i = e + 1;
  }
  if (!any) all_c = all_r = all_n = all_b = 0;
  uint32_t rule = 0;
  if (b < D->n) {
    int ns = b + 1, ne = lend(ns);
    if (is_rule_line(D->c + ns, ne - ns)) { int k = ns; while (k < ne && D->c[k] == ' ') k++; rule = D->c[k]; }
  }
  int off = 0;
  if ((kind == FMT_TITLE && rule == '=') || (kind == FMT_SUBTITLE && rule == '-' && all_c) || (kind == FMT_SECTION && rule == '-' && !all_c)) {
    int ne = lend(b + 1); /* знімаємо заголовок: прибираємо лінію й вирівнювання */
    b = ne;
    kind = FMT_LEFT;
    off = 1;
  } else if ((kind == FMT_CENTER && all_c) || (kind == FMT_RIGHT && all_r)) { kind = FMT_LEFT; off = 1; }
  else if ((kind == FMT_NUMBERS && all_n) || (kind == FMT_BULLETS && all_b)) { kind = FMT_UNLIST; off = 1; }
  if (off) set_toast("Formatting removed");
  int under = kind == FMT_TITLE || kind == FMT_SUBTITLE || kind == FMT_SECTION || off;
  /* якщо під заголовком уже є лінія з «=» чи «-», замінюємо її, а не додаємо ще одну */
  if (under && !off && b < D->n) {
    int ns = b + 1, ne = ns;
    while (ne < D->n && D->c[ne] != '\n') ne++;
    if (is_rule_line(D->c + ns, ne - ns)) b = ne;
  }
  under = kind == FMT_TITLE || kind == FMT_SUBTITLE || kind == FMT_SECTION;
  U32 o = {0};
  int maxw = 0, num = 1;
  for (int i = a; i <= b;) {
    int e = i;
    while (e < b && D->c[e] != '\n') e++;
    int ts = i, te = e;
    while (ts < te && D->c[ts] == ' ') ts++;
    while (te > ts && D->c[te - 1] == ' ') te--;
    int len = te - ts;
    if ((under || off) && is_rule_line(D->c + i, e - i) && i > a) { i = e + 1; continue; } /* стара лінія */
    if (o.n) u_put(&o, '\n');
    if (len > maxw) maxw = len;
    if (len == 0 && kind != FMT_LEFT) { /* порожні рядки лишаємо порожніми */ }
    else if (kind == FMT_TITLE || kind == FMT_SUBTITLE || kind == FMT_CENTER) {
      u_rep(&o, ' ', (COLS - len) / 2 > 0 ? (COLS - len) / 2 : 0);
      for (int k = ts; k < te; k++) u_put(&o, D->c[k]); /* регістр не чіпаємо: заголовок — це центр і лінія */
    } else if (kind == FMT_RIGHT) {
      u_rep(&o, ' ', COLS - len > 0 ? COLS - len : 0);
      u_arr(&o, D->c + ts, len);
    } else if (kind == FMT_LEFT || kind == FMT_SECTION) {
      u_arr(&o, D->c + ts, len);
    } else if (kind == FMT_UNLIST) {
      int li, num, ml;
      if (list_item(i, e, &li, &num, &ml)) { u_rep(&o, ' ', li); u_arr(&o, D->c + i + li + ml, e - (i + li + ml)); }
      else u_arr(&o, D->c + i, e - i);
    } else if (kind == FMT_NUMBERS || kind == FMT_BULLETS) {
      int ind = ts - i, k = ts; /* відступ лишаємо, старий маркер списку знімаємо */
      while (k < te && D->c[k] >= '0' && D->c[k] <= '9') k++;
      if (k > ts && k + 1 < te && D->c[k] == '.' && D->c[k + 1] == ' ') ts = k + 2;
      else if (te - ts >= 2 && (D->c[ts] == '-' || D->c[ts] == '*') && D->c[ts + 1] == ' ') ts += 2;
      u_rep(&o, ' ', ind);
      char m[16];
      if (kind == FMT_NUMBERS) snprintf(m, sizeof m, "%d. ", num++); else snprintf(m, sizeof m, "- ");
      u_str(&o, m);
      u_arr(&o, D->c + ts, te - ts);
    }
    if (e >= b) break;
    i = e + 1;
  }
  if (under && maxw) {
    u_put(&o, '\n');
    int pad = kind == FMT_SECTION ? 0 : (COLS - maxw) / 2;
    u_rep(&o, ' ', pad > 0 ? pad : 0);
    u_rep(&o, kind == FMT_TITLE ? '=' : '-', maxw);
  }
  replace_block(a, b, &o, -1);
}

/* ---------- дата й час ---------- */
static const char *TZS[] = {"", "UTC", "Europe/Kyiv", "Europe/Warsaw", "Europe/Berlin", "Europe/London",
                            "America/New_York", "America/Los_Angeles", "Asia/Tokyo", "Asia/Shanghai", "Australia/Sydney"};
static const char *TZ_LBL[] = {"Local", "UTC", "Kyiv", "Warsaw", "Berlin", "London", "New York", "Los Angeles", "Tokyo", "Shanghai", "Sydney"};
#define NTZ ((int)(sizeof TZS / sizeof TZS[0]))
static const char *MON_UK[] = {"January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"};
static const char *DAY_UK[] = {"Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"};
#define NDATEFMT 13

static void date_format(int f, char *out, size_t n) {
  time_t t = time(NULL);
  struct tm tm;
  const char *z = S.tzcustom[0] ? S.tzcustom : (S.tz >= 0 && S.tz < NTZ ? TZS[S.tz] : "");
  char *old = getenv("TZ") ? strdup(getenv("TZ")) : NULL;
  if (*z) { setenv("TZ", z, 1); tzset(); }
  localtime_r(&t, &tm);
  char zone[32];
  snprintf(zone, sizeof zone, "%s", tm.tm_zone ? tm.tm_zone : "");
  long off = tm.tm_gmtoff;
  char b[128] = "";
  switch (f) {
    case 0: strftime(b, sizeof b, "%d.%m.%Y", &tm); break;
    case 1: strftime(b, sizeof b, "%Y-%m-%d", &tm); break;
    case 2: snprintf(b, sizeof b, "%s %d, %d", MON_UK[tm.tm_mon], tm.tm_mday, tm.tm_year + 1900); break;
    case 3: snprintf(b, sizeof b, "%s, %s %d, %d", DAY_UK[tm.tm_wday], MON_UK[tm.tm_mon], tm.tm_mday, tm.tm_year + 1900); break;
    case 4: strftime(b, sizeof b, "%-d %B %Y", &tm); break;
    case 5: strftime(b, sizeof b, "%B %-d, %Y", &tm); break;
    case 6: strftime(b, sizeof b, "%H:%M", &tm); break;
    case 7: strftime(b, sizeof b, "%H:%M:%S", &tm); break;
    case 8: strftime(b, sizeof b, "%d.%m.%Y %H:%M", &tm); break;
    case 9: strftime(b, sizeof b, "%Y-%m-%d %H:%M %Z", &tm); break;
    case 10: {
      char d[64];
      strftime(d, sizeof d, "%Y-%m-%dT%H:%M:%S", &tm);
      long a = off < 0 ? -off : off;
      if (!off) snprintf(b, sizeof b, "%sZ", d);
      else snprintf(b, sizeof b, "%s%c%02ld:%02ld", d, off < 0 ? '-' : '+', a / 3600, a % 3600 / 60);
      break;
    }
    case 11: strftime(b, sizeof b, "%a, %d %b %Y %H:%M:%S %z", &tm); break;
    case 12: snprintf(b, sizeof b, "%lld", (long long)t); break;
  }
  if (*z) { if (old) setenv("TZ", old, 1); else unsetenv("TZ"); tzset(); }
  free(old);
  (void)zone;
  snprintf(out, n, "%s", b);
}

static void insert_utf8(const char *s) {
  if (ro_block()) return;
  int n;
  uint32_t *c = utf8_dec(s, strlen(s), &n);
  if (n) insert(c, n, 0);
  free(c);
}

static void insert_date(int f) { char b[128]; date_format(f, b, sizeof b); insert_utf8(b); }

static void insert_author(void) {
  if (ro_block()) return;
  if (!S.author[0]) { set_toast("Set the author's name in Settings (Ctrl+,)"); return; }
  int a, b;
  para_range(&a, &b);
  U32 o = {0};
  int n;
  uint32_t *c = utf8_dec(S.author, strlen(S.author), &n);
  if (b > a) { u_arr(&o, D->c + a, b - a); u_put(&o, '\n'); a = b; }
  u_rep(&o, ' ', COLS - n > 0 ? COLS - n : 0);
  u_arr(&o, c, n);
  free(c);
  replace_block(a, a, &o, -1);
}

/* Окремий рядок тексту на місці курсора (по центру або ліворуч). */
static void insert_own_line(const char *t, int center) {
  if (ro_block()) return;
  int n;
  uint32_t *c = utf8_dec(t, strlen(t), &n);
  U32 o = {0};
  int a = D->cur < D->anchor ? D->cur : D->anchor, b = D->cur < D->anchor ? D->anchor : D->cur;
  if (a > 0 && D->c[a - 1] != '\n') u_put(&o, '\n');
  if (center) u_rep(&o, ' ', (COLS - n) / 2 > 0 ? (COLS - n) / 2 : 0);
  u_arr(&o, c, n);
  u_put(&o, '\n');
  free(c);
  replace_block(a, b, &o, -1);
}

static int is_border(const uint32_t *c, int n) {
  int i = 0;
  while (i < n && c[i] == ' ') i++;
  if (i >= n || c[i] != '+') return 0;
  for (; i < n; i++) if (c[i] != '+' && c[i] != '-' && c[i] != '=' && c[i] != ':' && c[i] != ' ') return 0;
  return 1;
}
static int has_pipe(const uint32_t *c, int n) { for (int i = 0; i < n; i++) if (c[i] == '|') return 1; return 0; }

/* ================= живі таблиці =================
 * У файлі таблиця — звичайний текст:
 *   +--------+--------+
 *   | Ім'я   | Місто  |
 *   +========+========+
 *   | Олена  | Київ   |
 *   +--------+--------+
 * Коли курсор у ній, програма розбирає її на клітинки, і кожна дія (друк, видалення,
 * вставка рядка чи стовпця, зміна ширини) змінює клітинку, після чого таблиця одразу
 * перемальовується: стовпці ширшають за текстом, поки є місце на сторінці, далі текст
 * переноситься всередині клітинки. Рамку зламати не можна — її малює програма. */
typedef struct { int nr, nc, header, *w; U32 *cell; } Tbl;
#define TCELL(t, r, c) (&(t)->cell[(r) * (t)->nc + (c)])
#define TMINW 3

static void tbl_free(Tbl *t) {
  for (int i = 0; i < t->nr * t->nc; i++) free(t->cell[i].p);
  free(t->cell); free(t->w);
  memset(t, 0, sizeof *t);
}

/* межі рядка тексту, що містить p: [ls, le) */
static void line_bounds(int p, int *ls, int *le) { *ls = lstart(p); *le = lend(p); }

/* Рядок таблиці: 0 — ні, 1 — рамка «+--+», 2 — вміст «| |». ind — відступ. */
static int tline_kind(int ls, int le, int *ind) {
  int i = ls;
  while (i < le && D->c[i] == ' ') i++;
  *ind = i - ls;
  int e = le;
  while (e > i && D->c[e - 1] == ' ') e--;
  if (e - i < 3) return 0;
  if (D->c[i] == '+' && D->c[e - 1] == '+') {
    for (int k = i; k < e; k++) if (D->c[k] != '+' && D->c[k] != '-' && D->c[k] != '=') return 0;
    return 1;
  }
  if (D->c[i] == '|' && D->c[e - 1] == '|') return 2;
  return 0;
}

/* Знаходить таблицю навколо позиції p. a — початок першого рядка, b — кінець останнього (без \n). */
static int tbl_block(int p, int *a, int *b, int *indent) {
  if (!D || D->ro || is_media_view(D->view)) return 0;
  int ls, le, ind;
  line_bounds(p, &ls, &le);
  if (!tline_kind(ls, le, &ind)) return 0;
  int A = ls, B = le, ind0 = ind;
  while (A > 0) {
    int pls, ple, pind;
    line_bounds(A - 1, &pls, &ple);
    if (!tline_kind(pls, ple, &pind) || pind != ind0) break;
    A = pls;
  }
  while (B < D->n) {
    int nls, nle, nind;
    line_bounds(B + 1, &nls, &nle);
    if (!tline_kind(nls, nle, &nind) || nind != ind0) break;
    B = nle;
  }
  /* перший і останній рядки — рамки; «+» рамок і «|» вмісту на тих самих місцях */
  int fls, fle, find_, lls, lle, lind;
  line_bounds(A, &fls, &fle);
  line_bounds(B, &lls, &lle);
  if (tline_kind(fls, fle, &find_) != 1 || tline_kind(lls, lle, &lind) != 1) return 0;
  int cols[256], nc = 0;
  for (int k = fls + ind0; k < fle && nc < 256; k++) if (D->c[k] == '+') cols[nc++] = k - fls;
  if (nc < 2) return 0;
  for (int i = A; i <= B;) {
    int ls2, le2, ik;
    line_bounds(i, &ls2, &le2);
    int kind = tline_kind(ls2, le2, &ik);
    for (int q = 0; q < nc; q++) {
      int pos = ls2 + cols[q];
      if (pos >= le2) return 0;
      uint32_t ch = D->c[pos];
      if (kind == 1 ? ch != '+' : ch != '|') return 0;
    }
    if (le2 >= B) break;
    i = le2 + 1;
  }
  *a = A; *b = B; *indent = ind0;
  return 1;
}

static void tbl_cols(int a, int indent, int *cols, int *nc) {
  int ls, le;
  line_bounds(a, &ls, &le);
  *nc = 0;
  for (int k = ls + indent; k < le && *nc < 256; k++) if (D->c[k] == '+') cols[(*nc)++] = k - ls;
}

/* Розбирає таблицю [a, b] на клітинки. */
static void tbl_parse(int a, int b, int indent, Tbl *t) {
  int cols[256], nb;
  tbl_cols(a, indent, cols, &nb);
  memset(t, 0, sizeof *t);
  t->nc = nb - 1;
  t->w = malloc((size_t)t->nc * sizeof(int));
  for (int c = 0; c < t->nc; c++) { t->w[c] = cols[c + 1] - cols[c] - 3; if (t->w[c] < 1) t->w[c] = 1; }
  int capr = 0, inrow = 0, borders = 0, lastlen[256] = {0};
  for (int i = a; i <= b;) {
    int ls, le, ik;
    line_bounds(i, &ls, &le);
    int kind = tline_kind(ls, le, &ik);
    if (kind == 1) {
      borders++;
      if (borders == 2 && t->nr == 1) for (int k = ls + cols[0]; k < le; k++) if (D->c[k] == '=') { t->header = 1; break; }
      inrow = 0;
    } else {
      if (!inrow) { /* новий рядок таблиці */
        if (t->nr == capr) { capr = capr ? capr * 2 : 8; t->cell = realloc(t->cell, (size_t)capr * t->nc * sizeof(U32)); }
        memset(&t->cell[t->nr * t->nc], 0, (size_t)t->nc * sizeof(U32));
        t->nr++;
        inrow = 1;
        memset(lastlen, 0, sizeof lastlen);
      }
      for (int c = 0; c < t->nc; c++) {
        int s = ls + cols[c] + 1, e = ls + cols[c + 1];
        while (s < e && D->c[s] == ' ') s++;
        while (e > s && D->c[e - 1] == ' ') e--;
        if (e > s) {
          U32 *cl = TCELL(t, t->nr - 1, c);
          /* відрізки клітинки з'єднуються пробілом; після відрізка на всю ширину — без
             пробілу: це довге слово, розрізане на межі клітинки */
          if (cl->n && lastlen[c] != t->w[c]) u_put(cl, ' ');
          u_arr(cl, D->c + s, e - s);
        }
        lastlen[c] = e - s;
      }
    }
    if (le >= b) break;
    i = le + 1;
  }
  if (!t->nr) { t->nr = 1; t->cell = calloc((size_t)t->nc, sizeof(U32)); }
}

/* Перенесення тексту клітинки по словах: початки й довжини відрізків. */
static int tbl_wrap(const U32 *s, int w, int *st, int *ln, int max) {
  int n = 0, i = 0;
  if (s->n == 0) { st[0] = 0; ln[0] = 0; return 1; }
  while (i < s->n && n < max) {
    int rem = s->n - i;
    if (rem <= w) { st[n] = i; ln[n] = rem; n++; break; }
    int br = -1;
    for (int k = i + w; k > i; k--) if (s->p[k] == ' ') { br = k; break; }
    if (br < 0) { st[n] = i; ln[n] = w; n++; i += w; }   /* довге слово — ріжемо */
    else { st[n] = i; ln[n] = br - i; n++; i = br + 1; } /* пробіл на межі ковтається */
    if (i >= s->n && n < max) { /* текст закінчився пробілом — курсор після нього на новому рядку */
      if (s->n && s->p[s->n - 1] == ' ' && br >= 0) { st[n] = s->n; ln[n] = 0; n++; }
    }
  }
  return n;
}

static int tbl_maxlen(Tbl *t, int c) { int m = 0; for (int r = 0; r < t->nr; r++) if (TCELL(t, r, c)->n > m) m = TCELL(t, r, c)->n; return m; }
static int tbl_width(Tbl *t, int indent) { int s = indent + 1; for (int c = 0; c < t->nc; c++) s += t->w[c] + 3; return s; }

/* Таблиця не ширша за сторінку: звужуємо найширші стовпці. */
static void tbl_fit_page(Tbl *t, int indent) {
  while (tbl_width(t, indent) > COLS) {
    int wi = -1;
    for (int c = 0; c < t->nc; c++) if (t->w[c] > TMINW && (wi < 0 || t->w[c] > t->w[wi])) wi = c;
    if (wi < 0) break;
    t->w[wi]--;
  }
}

/* Ширина стовпця c після зміни тексту: росте за текстом, поки є місце; підтягнутий стовпець
   і зменшується разом із текстом, а ширину, задану вручну, не чіпаємо. */
static void tbl_autowidth(Tbl *t, int c, int old_max, int indent, int cell_len) {
  int need = tbl_maxlen(t, c);
  if (need < TMINW) need = TMINW;
  if (cell_len > t->w[c]) { /* росте лише від тексту, який зараз пишеться, — звужений вручну стовпець не розпирає */
    need = cell_len;
    int avail = COLS - tbl_width(t, indent);
    if (avail > 0) t->w[c] += need - t->w[c] < avail ? need - t->w[c] : avail;
  } else if (need < t->w[c] && old_max >= t->w[c]) t->w[c] = need;
  tbl_fit_page(t, indent);
}

/* Малює таблицю текстом; caret_off — місце курсора для клітинки (cr, cc), зсуву ck. */
static void tbl_render(Tbl *t, int indent, U32 *o, int cr, int cc, int ck, int *caret_off) {
  static int st[256 * 64], ln[256 * 64];
  *caret_off = -1;
#define TB(ch) do { u_rep(o, ' ', indent); u_put(o, '+'); for (int c_ = 0; c_ < t->nc; c_++) { u_rep(o, ch, t->w[c_] + 2); u_put(o, '+'); } } while (0)
  TB('-');
  for (int r = 0; r < t->nr; r++) {
    int nl[256] = {0}, h = 1, cli = -1;
    for (int c = 0; c < t->nc; c++) {
      nl[c] = tbl_wrap(TCELL(t, r, c), t->w[c], st + c * 64, ln + c * 64, 64);
      if (nl[c] > h) h = nl[c];
    }
    if (r == cr && cc >= 0 && cc < t->nc) { /* у якому перенесеному рядку клітинки стоїть курсор */
      cli = 0;
      for (int li = 0; li < nl[cc]; li++) if (st[cc * 64 + li] <= ck) cli = li;
    }
    for (int li = 0; li < h; li++) {
      u_put(o, '\n');
      u_rep(o, ' ', indent);
      u_put(o, '|');
      for (int c = 0; c < t->nc; c++) {
        u_put(o, ' ');
        int len = li < nl[c] ? ln[c * 64 + li] : 0, s0 = li < nl[c] ? st[c * 64 + li] : 0;
        if (c == cc && li == cli) { int d = ck - s0; if (d > len) d = len; if (d < 0) d = 0; *caret_off = o->n + d; }
        if (len) u_arr(o, TCELL(t, r, c)->p + s0, len);
        u_rep(o, ' ', t->w[c] - len + 1);
        u_put(o, '|');
      }
    }
    u_put(o, '\n');
    TB(r == 0 && t->header && t->nr > 1 ? '=' : '-');
  }
#undef TB
}

/* Позиція в буфері → клітинка й зсув у її тексті (з урахуванням перенесених рядків клітинки). */
static void tbl_locate(int a, int b, int indent, int pos, int *rr, int *rc, int *rk) {
  int cols[256], nb;
  tbl_cols(a, indent, cols, &nb);
  int nc = nb - 1, r = -1, prev_border = 0, nrows = 0;
  int kpos[256], plen[256];
  *rr = 0; *rc = 0; *rk = 0;
  /* скільки рядків таблиці всього (для рамки внизу) */
  for (int i = a, pb = 0; i <= b;) {
    int ls, le, ik;
    line_bounds(i, &ls, &le);
    int kind = tline_kind(ls, le, &ik);
    if (kind == 2 && pb) nrows++;
    pb = kind == 1;
    if (le >= b) break;
    i = le + 1;
  }
  for (int i = a; i <= b;) {
    int ls, le, ik;
    line_bounds(i, &ls, &le);
    int kind = tline_kind(ls, le, &ik);
    int col = pos - ls, c = 0;
    for (int q = 0; q < nc; q++) if (col > cols[q]) c = q;
    if (kind == 1) {
      if (pos >= ls && pos <= le) { *rr = r + 1 < nrows ? r + 1 : (r < 0 ? 0 : r); *rc = c; *rk = 0; return; }
      prev_border = 1;
    } else {
      if (prev_border) { r++; for (int q = 0; q < nc; q++) kpos[q] = plen[q] = 0; }
      prev_border = 0;
      for (int q = 0; q < nc; q++) {
        int s = ls + cols[q] + 1, e = ls + cols[q + 1];
        while (s < e && D->c[s] == ' ') s++;
        int te = e;
        while (te > s && D->c[te - 1] == ' ') te--;
        int wq = cols[q + 1] - cols[q] - 3;
        int len = te - s, base = kpos[q] + (kpos[q] && len && plen[q] != wq ? 1 : 0); /* як у tbl_parse */
        if (q == c && pos >= ls && pos <= le) {
          int off = pos - s;
          if (off < 0) off = 0;
          if (off > len) off = len;
          *rr = r; *rc = c; *rk = base + off;
          return;
        }
        if (len) kpos[q] = base + len;
        plen[q] = len;
      }
    }
    if (le >= b) break;
    i = le + 1;
  }
}

/* Поточна таблиця під курсором. */
/* Модель зберігається між натисканнями: так не губляться пробіли в кінці клітинки,
   поки ви пишете. Якщо текст змінено інакше — розбираємо таблицю заново. */
static struct { int a, b, indent; Tbl t; int r, c, k; Doc *doc; unsigned ver; int cur, valid; } TC;
static int tbl_here(void) {
  if (!D || D->ro) return 0;
  if (TC.valid && TC.doc == D && TC.ver == D->ver && D->cur >= TC.a && D->cur <= TC.b) {
    if (D->cur != TC.cur) { tbl_locate(TC.a, TC.b, TC.indent, D->cur, &TC.r, &TC.c, &TC.k); TC.cur = D->cur; }
    return 1;
  }
  TC.valid = 0;
  if (!tbl_block(D->cur, &TC.a, &TC.b, &TC.indent)) return 0;
  TC.valid = 1; TC.doc = D; TC.ver = D->ver; TC.cur = D->cur;
  tbl_free(&TC.t);
  tbl_parse(TC.a, TC.b, TC.indent, &TC.t);
  tbl_locate(TC.a, TC.b, TC.indent, D->cur, &TC.r, &TC.c, &TC.k);
  if (TC.r >= TC.t.nr) TC.r = TC.t.nr - 1;
  if (TC.c >= TC.t.nc) TC.c = TC.t.nc - 1;
  U32 *cl = TCELL(&TC.t, TC.r, TC.c);
  if (TC.k > cl->n) TC.k = cl->n;
  return 1;
}

/* Записує змінену таблицю назад у текст і ставить курсор у клітинку (r, c, k). */
static void tbl_commit(int kind) {
  U32 o = {0};
  int off;
  tbl_fit_page(&TC.t, TC.indent);
  tbl_render(&TC.t, TC.indent, &o, TC.r, TC.c, TC.k, &off);
  before_edit(kind);
  replace_range(TC.a, TC.b, o.p, o.n);
  D->cur = TC.a + (off >= 0 ? off : 0);
  after_edit();
  TC.b = TC.a + o.n;
  TC.ver = D->ver; TC.cur = D->cur; TC.doc = D; TC.valid = 1;
  free(o.p);
}

static void tbl_move_caret(void) { /* лише курсор, без зміни тексту */
  U32 o = {0};
  int off;
  tbl_render(&TC.t, TC.indent, &o, TC.r, TC.c, TC.k, &off);
  free(o.p);
  D->cur = D->anchor = TC.a + (off >= 0 ? off : 0);
  D->desired_col = -1;
  need_visible = 1;
  TC.cur = D->cur;
}

static void tbl_add_row(int at) {
  Tbl *t = &TC.t;
  t->cell = realloc(t->cell, (size_t)(t->nr + 1) * t->nc * sizeof(U32));
  memmove(&t->cell[(at + 1) * t->nc], &t->cell[at * t->nc], (size_t)(t->nr - at) * t->nc * sizeof(U32));
  memset(&t->cell[at * t->nc], 0, (size_t)t->nc * sizeof(U32));
  t->nr++;
}

static void tbl_add_col(int at) {
  Tbl *t = &TC.t;
  if (t->nc >= 200) return;
  U32 *nc = calloc((size_t)t->nr * (t->nc + 1), sizeof(U32));
  for (int r = 0; r < t->nr; r++)
    for (int c = 0, d = 0; c <= t->nc; c++) {
      if (c == at) continue;
      nc[r * (t->nc + 1) + c] = t->cell[r * t->nc + d++];
    }
  free(t->cell);
  t->cell = nc;
  t->w = realloc(t->w, (size_t)(t->nc + 1) * sizeof(int));
  memmove(&t->w[at + 1], &t->w[at], (size_t)(t->nc - at) * sizeof(int));
  t->w[at] = 8;
  t->nc++;
}

/* Дії з таблицею (рядки, стовпці, ширина). 1 — курсор у таблиці й дію виконано. */
static int tbl_op(int op) {
  if (!tbl_here()) { set_toast("The cursor is not in a table"); return 0; }
  Tbl *t = &TC.t;
  switch (op) {
    case A_ROW_BELOW: tbl_add_row(TC.r + 1); TC.r++; TC.k = 0; break;
    case A_ROW_ABOVE: tbl_add_row(TC.r); TC.k = 0; break;
    case A_COL_RIGHT: tbl_add_col(TC.c + 1); TC.c++; TC.k = 0; break;
    case A_COL_LEFT: tbl_add_col(TC.c); TC.k = 0; break;
    case A_ROW_DEL:
      if (t->nr == 1) { set_toast("This is the last row of the table"); return 1; }
      for (int c = 0; c < t->nc; c++) free(TCELL(t, TC.r, c)->p);
      memmove(&t->cell[TC.r * t->nc], &t->cell[(TC.r + 1) * t->nc], (size_t)(t->nr - TC.r - 1) * t->nc * sizeof(U32));
      t->nr--;
      if (TC.r >= t->nr) TC.r = t->nr - 1;
      TC.k = 0;
      break;
    case A_COL_DEL: {
      if (t->nc == 1) { set_toast("This is the last column of the table"); return 1; }
      U32 *nc = calloc((size_t)t->nr * (t->nc - 1), sizeof(U32));
      for (int r = 0; r < t->nr; r++)
        for (int c = 0, d = 0; c < t->nc; c++) {
          if (c == TC.c) { free(t->cell[r * t->nc + c].p); continue; }
          nc[r * (t->nc - 1) + d++] = t->cell[r * t->nc + c];
        }
      free(t->cell);
      t->cell = nc;
      memmove(&t->w[TC.c], &t->w[TC.c + 1], (size_t)(t->nc - TC.c - 1) * sizeof(int));
      t->nc--;
      if (TC.c >= t->nc) TC.c = t->nc - 1;
      TC.k = 0;
      break;
    }
    case A_COL_WIDER:
      if (tbl_width(t, TC.indent) >= COLS) { set_toast("The table already fills the page width"); return 1; }
      t->w[TC.c]++;
      break;
    case A_COL_NARROWER:
      if (t->w[TC.c] > TMINW) t->w[TC.c]--;
      break;
  }
  U32 *cl = TCELL(t, TC.r, TC.c);
  if (TC.k > cl->n) TC.k = cl->n;
  tbl_commit(0);
  return 1;
}

static int tbl_erase(int forward, int word);
/* Друк у клітинці. Повертає 1, якщо курсор був у таблиці. */
static int tbl_type(const uint32_t *s, int n) {
  if (!tbl_here()) return 0;
  if (D->cur != D->anchor) { /* виділення: якщо воно всередині таблиці — спершу видаляємо */
    int a = sel_a(), b = sel_b();
    if (a < TC.a || b > TC.b) { set_toast("The selection covers part of a table: select all of it, or only the text of the cells"); return 1; }
    tbl_erase(0, 0); /* спершу прибираємо виділений текст клітинок */
    if (!tbl_here()) return 1;
  }
  int multi = 0;
  for (int i = 0; i < n; i++) if (s[i] == '\t' || s[i] == '\n') multi = 1;
  if (multi) { /* вставка з таблиці (табуляції й рядки) — заповнюємо клітинки, додаючи рядки й стовпці */
    int r = TC.r, c = TC.c;
    U32 *cl = TCELL(&TC.t, r, c);
    for (int i = 0; i < n; i++) {
      if (s[i] == '\t') {
        c++;
        if (c >= TC.t.nc) { int om = 0; tbl_add_col(c); TC.t.w[c] = TMINW; (void)om; }
        cl = TCELL(&TC.t, r, c);
      } else if (s[i] == '\n') {
        if (i == n - 1) break;
        r++; c = TC.c;
        if (r >= TC.t.nr) tbl_add_row(r);
        cl = TCELL(&TC.t, r, c);
      } else u_put(cl, s[i]);
    }
    for (int q = 0; q < TC.t.nc; q++) tbl_autowidth(&TC.t, q, 0, TC.indent, tbl_maxlen(&TC.t, q));
    TC.r = r; TC.c = c; TC.k = TCELL(&TC.t, r, c)->n;
    tbl_commit(0);
    return 1;
  }
  int old = tbl_maxlen(&TC.t, TC.c);
  U32 *cl = TCELL(&TC.t, TC.r, TC.c);
  for (int i = 0; i < n; i++) u_put(cl, 0);
  memmove(cl->p + TC.k + n, cl->p + TC.k, (size_t)(cl->n - n - TC.k) * sizeof(uint32_t));
  memcpy(cl->p + TC.k, s, (size_t)n * sizeof(uint32_t));
  TC.k += n;
  tbl_autowidth(&TC.t, TC.c, old, TC.indent, TCELL(&TC.t, TC.r, TC.c)->n);
  tbl_commit(n == 1 ? 1 : 0);
  return 1;
}

/* Backspace / Delete у клітинці: рамку не чіпають. */
static int tbl_erase(int forward, int word) {
  if (!tbl_here()) return 0;
  if (D->cur != D->anchor) {
    int a = sel_a(), b = sel_b();
    if (a <= TC.a && b >= TC.b) return 0; /* виділено всю таблицю — звичайне видалення */
    if (a < TC.a || b > TC.b) { set_toast("The selection covers part of a table: select all of it, or only the text of the cells"); return 1; }
    int r1, c1, k1, r2, c2, k2;
    tbl_locate(TC.a, TC.b, TC.indent, a, &r1, &c1, &k1);
    tbl_locate(TC.a, TC.b, TC.indent, b, &r2, &c2, &k2);
    for (int r = r1; r <= r2 && r < TC.t.nr; r++)
      for (int c = 0; c < TC.t.nc; c++) {
        int first = r == r1 && c == c1, last = r == r2 && c == c2;
        if ((r == r1 && c < c1) || (r == r2 && c > c2)) continue;
        U32 *cl = TCELL(&TC.t, r, c);
        int s0 = first ? k1 : 0, e0 = last ? k2 : cl->n;
        if (s0 > cl->n) s0 = cl->n;
        if (e0 > cl->n) e0 = cl->n;
        if (e0 > s0) { memmove(cl->p + s0, cl->p + e0, (size_t)(cl->n - e0) * sizeof(uint32_t)); cl->n -= e0 - s0; }
      }
    TC.r = r1; TC.c = c1; TC.k = k1;
    for (int q = 0; q < TC.t.nc; q++) tbl_autowidth(&TC.t, q, TC.t.w[q], TC.indent, 0);
    tbl_commit(0);
    return 1;
  }
  U32 *cl = TCELL(&TC.t, TC.r, TC.c);
  int old = tbl_maxlen(&TC.t, TC.c), s0, e0;
  if (forward) {
    if (TC.k >= cl->n) return 1;
    s0 = TC.k; e0 = TC.k + 1;
    if (word) { while (e0 < cl->n && cl->p[e0] == ' ') e0++; while (e0 < cl->n && cl->p[e0] != ' ') e0++; }
  } else {
    if (TC.k == 0) return 1; /* на початку клітинки нічого не робимо — рамка ціла */
    e0 = TC.k; s0 = TC.k - 1;
    if (word) { while (s0 > 0 && cl->p[s0 - 1] == ' ') s0--; while (s0 > 0 && cl->p[s0 - 1] != ' ') s0--; }
  }
  memmove(cl->p + s0, cl->p + e0, (size_t)(cl->n - e0) * sizeof(uint32_t));
  cl->n -= e0 - s0;
  TC.k = s0;
  tbl_autowidth(&TC.t, TC.c, old, TC.indent, TCELL(&TC.t, TC.r, TC.c)->n);
  tbl_commit(word ? 0 : 2);
  return 1;
}

/* Курсор після таблиці: на початок наступного рядка, а якщо таблиця в кінці — створюємо його. */
static void tbl_leave_down(void) {
  if (TC.b >= D->n) { uint32_t nl = '\n'; D->cur = D->anchor = D->n; insert(&nl, 1, 0); return; }
  D->cur = D->anchor = TC.b + 1;
  need_visible = 1;
}

/* Навігація й Enter/Tab у таблиці. Повертає 1, якщо клавішу оброблено. */
static int tbl_key(SDL_Keysym ks) {
  int shift = ks.mod & KMOD_SHIFT, cmd = ks.mod & CMDMOD, alt = ks.mod & KMOD_ALT;
  if (alt) return 0;
  SDL_Keycode k = ks.sym;
  if (k != SDLK_TAB && k != SDLK_RETURN && k != SDLK_KP_ENTER && k != SDLK_UP && k != SDLK_DOWN && k != SDLK_LEFT && k != SDLK_RIGHT &&
      k != SDLK_HOME && k != SDLK_END) return 0;
  if (shift && k != SDLK_TAB && k != SDLK_RETURN) return 0; /* виділення стрілками — звичайне */
  if (cmd && (k == SDLK_HOME || k == SDLK_END || k == SDLK_LEFT || k == SDLK_RIGHT)) return 0;
  if (!tbl_here()) return 0;
  Tbl *t = &TC.t;
  U32 *cl = TCELL(t, TC.r, TC.c);
  switch (k) {
    case SDLK_TAB:
      if (shift) { if (TC.c > 0) TC.c--; else if (TC.r > 0) { TC.r--; TC.c = t->nc - 1; } TC.k = TCELL(t, TC.r, TC.c)->n; tbl_move_caret(); return 1; }
      if (TC.c + 1 < t->nc) TC.c++;
      else if (TC.r + 1 < t->nr) { TC.r++; TC.c = 0; }
      else { tbl_add_row(t->nr); TC.r++; TC.c = 0; TC.k = 0; tbl_commit(0); return 1; } /* Tab в останній клітинці — новий рядок */
      TC.k = TCELL(t, TC.r, TC.c)->n;
      tbl_move_caret();
      return 1;
    case SDLK_RETURN: case SDLK_KP_ENTER:
      if (shift) { if (TC.r > 0) { TC.r--; TC.k = TCELL(t, TC.r, TC.c)->n; tbl_move_caret(); } return 1; }
      if (TC.r + 1 < t->nr) { TC.r++; TC.k = TCELL(t, TC.r, TC.c)->n; tbl_move_caret(); return 1; }
      tbl_add_row(t->nr); TC.r++; TC.k = 0; tbl_commit(0); /* Enter в останньому рядку — новий рядок */
      return 1;
    case SDLK_UP:
      if (TC.r == 0) { if (TC.a > 0) { D->cur = D->anchor = TC.a - 1; need_visible = 1; } return 1; }
      TC.r--;
      if (TC.k > TCELL(t, TC.r, TC.c)->n) TC.k = TCELL(t, TC.r, TC.c)->n;
      tbl_move_caret();
      return 1;
    case SDLK_DOWN:
      if (TC.r + 1 >= t->nr) { tbl_leave_down(); return 1; }
      TC.r++;
      if (TC.k > TCELL(t, TC.r, TC.c)->n) TC.k = TCELL(t, TC.r, TC.c)->n;
      tbl_move_caret();
      return 1;
    case SDLK_LEFT:
      if (TC.k > 0) TC.k--;
      else if (TC.c > 0) { TC.c--; TC.k = TCELL(t, TC.r, TC.c)->n; }
      else if (TC.r > 0) { TC.r--; TC.c = t->nc - 1; TC.k = TCELL(t, TC.r, TC.c)->n; }
      else { if (TC.a > 0) { D->cur = D->anchor = TC.a - 1; need_visible = 1; } return 1; }
      tbl_move_caret();
      return 1;
    case SDLK_RIGHT:
      if (TC.k < cl->n) TC.k++;
      else if (TC.c + 1 < t->nc) { TC.c++; TC.k = 0; }
      else if (TC.r + 1 < t->nr) { TC.r++; TC.c = 0; TC.k = 0; }
      else { tbl_leave_down(); return 1; }
      tbl_move_caret();
      return 1;
    case SDLK_HOME: TC.k = 0; tbl_move_caret(); return 1;
    case SDLK_END: TC.k = cl->n; tbl_move_caret(); return 1;
  }
  return 0;
}

/* Після клацання мишкою курсор не стоїть на рамці — переносимо його в клітинку. */
static void tbl_snap(void) {
  if (D->cur != D->anchor || !tbl_here()) return;
  tbl_move_caret();
}

/* Рядкові дії над таблицею: Ctrl+D — копія рядка, Ctrl+Shift+K — видалити рядок, Alt+↑/↓ — пересунути. */
static int tbl_line_op(int op) {
  if (D->cur != D->anchor || !tbl_here()) return 0;
  Tbl *t = &TC.t;
  if (op == A_DUP) {
    tbl_add_row(TC.r + 1);
    for (int c = 0; c < t->nc; c++) { U32 *s = TCELL(t, TC.r, c), *d = TCELL(t, TC.r + 1, c); u_arr(d, s->p, s->n); }
    TC.r++;
  } else if (op == A_DEL_LINE) return tbl_op(A_ROW_DEL);
  else if (op == A_MOVE_UP || op == A_MOVE_DOWN) {
    int o = op == A_MOVE_UP ? TC.r - 1 : TC.r + 1;
    if (o < 0 || o >= t->nr) return 1;
    for (int c = 0; c < t->nc; c++) { U32 tmp = *TCELL(t, TC.r, c); *TCELL(t, TC.r, c) = *TCELL(t, o, c); *TCELL(t, o, c) = tmp; }
    TC.r = o;
  }
  tbl_commit(0);
  return 1;
}

/* Нова таблиця rows × cols на місці курсора (на окремих рядках). */
static void tbl_insert_new(int rows, int cols) {
  if (ro_block()) return;
  if (rows < 1) rows = 1;
  if (cols < 1) cols = 1;
  Tbl t = {0};
  t.nr = rows; t.nc = cols; t.header = rows > 1;
  t.w = malloc((size_t)cols * sizeof(int));
  t.cell = calloc((size_t)rows * cols, sizeof(U32));
  int w = (COLS - 1) / cols - 3;
  if (w > 10) w = 10;
  if (w < TMINW) w = TMINW;
  for (int c = 0; c < cols; c++) t.w[c] = w;
  tbl_fit_page(&t, 0);
  U32 o = {0};
  int a = sel_a(), b = sel_b(), off;
  if (a > 0 && D->c[a - 1] != '\n') u_put(&o, '\n');
  int base = o.n;
  U32 body = {0};
  tbl_render(&t, 0, &body, 0, 0, 0, &off);
  u_arr(&o, body.p, body.n);
  if (b >= D->n || D->c[b] != '\n') u_put(&o, '\n');
  free(body.p);
  tbl_free(&t);
  replace_block(a, b, &o, base + (off >= 0 ? off : 0));
}

/* Перетворює рядки з «|» (або вже готову таблицю) на рівну живу таблицю. */
static void tbl_from_text(void) {
  if (ro_block()) return;
  if (tbl_here()) { /* уже таблиця — просто підганяємо ширини під текст */
    for (int c = 0; c < TC.t.nc; c++) { int m = tbl_maxlen(&TC.t, c); TC.t.w[c] = m < TMINW ? TMINW : m; }
    tbl_fit_page(&TC.t, TC.indent);
    tbl_commit(0);
    return;
  }
  int a, b;
  para_range(&a, &b);
  if (D->cur == D->anchor) { /* суміжні рядки з «|» */
    for (;;) {
      if (a == 0) break;
      int ps = lstart(a - 1);
      if (has_pipe(D->c + ps, a - 1 - ps)) a = ps; else break;
    }
    for (;;) {
      if (b >= D->n) break;
      int ne = lend(b + 1);
      if (has_pipe(D->c + b + 1, ne - b - 1)) b = ne; else break;
    }
  }
  Tbl t = {0};
  int capr = 0, indent = -1;
  for (int i = a; i <= b;) {
    int e = lend(i);
    if (has_pipe(D->c + i, e - i) && !is_border(D->c + i, e - i)) {
      int k = i;
      while (k < e && D->c[k] == ' ') k++;
      if (indent < 0) indent = k - i;
      int le = e;
      while (le > k && D->c[le - 1] == ' ') le--;
      if (k < le && D->c[k] == '|') k++;
      if (le > k && D->c[le - 1] == '|') le--;
      int cells = 1;
      for (int q = k; q < le; q++) if (D->c[q] == '|') cells++;
      if (cells > t.nc) { /* розширюємо всі рядки до нової кількості стовпців */
        U32 *nc = calloc((size_t)(capr ? capr : 1) * cells, sizeof(U32));
        for (int r = 0; r < t.nr; r++) for (int c = 0; c < t.nc; c++) nc[r * cells + c] = t.cell[r * t.nc + c];
        free(t.cell); t.cell = nc; t.nc = cells;
      }
      if (t.nr >= capr) {
        int ncap = capr ? capr * 2 : 8;
        U32 *nc = calloc((size_t)ncap * t.nc, sizeof(U32));
        if (t.cell) memcpy(nc, t.cell, (size_t)t.nr * t.nc * sizeof(U32));
        free(t.cell); t.cell = nc; capr = ncap;
      }
      int c = 0;
      for (int st0 = k;;) {
        int en = st0;
        while (en < le && D->c[en] != '|') en++;
        int t0 = st0, t1 = en;
        while (t0 < t1 && D->c[t0] == ' ') t0++;
        while (t1 > t0 && D->c[t1 - 1] == ' ') t1--;
        u_arr(&t.cell[t.nr * t.nc + c], D->c + t0, t1 - t0);
        c++;
        if (en >= le) break;
        st0 = en + 1;
      }
      t.nr++;
    }
    if (e >= b) break;
    i = e + 1;
  }
  if (!t.nr) { set_toast("No table here: separate cells with “|” or insert a new one"); free(t.cell); return; }
  if (indent < 0) indent = 0;
  t.header = t.nr > 1;
  t.w = malloc((size_t)t.nc * sizeof(int));
  for (int c = 0; c < t.nc; c++) { int m = tbl_maxlen(&t, c); t.w[c] = m < TMINW ? TMINW : m; }
  tbl_fit_page(&t, indent);
  U32 o = {0};
  int off;
  tbl_render(&t, indent, &o, 0, 0, 0, &off);
  tbl_free(&t);
  replace_block(a, b, &o, off >= 0 ? off : 0);
}

/* ================= шляхи й налаштування ================= */
static char toast[512];
static Uint32 toast_until;
/* Повідомлення показуються в рядку стану внизу правої панелі, нічого не спливає. */
static void set_toast(const char *msg) { snprintf(toast, sizeof toast, "%s", msg); toast_until = SDL_GetTicks(); }

static const char *basename_of(const char *p) { const char *s = strrchr(p, '/'); return s ? s + 1 : p; }

static void dirname_of(const char *p, char *out, size_t n) {
  snprintf(out, n, "%s", p);
  char *s = strrchr(out, '/');
  if (s && s != out) *s = 0;
  else if (s) s[1] = 0;
  else snprintf(out, n, ".");
}

static int is_dir(const char *p) { struct stat st; return stat(p, &st) == 0 && S_ISDIR(st.st_mode); }
static int file_exists(const char *p) { struct stat st; return stat(p, &st) == 0 && S_ISREG(st.st_mode); }

static void home_dir(char *out, size_t n, const char *sub) {
  const char *h = getenv("HOME");
  if (!h) h = ".";
  if (sub) { snprintf(out, n, "%s/%s", h, sub); if (is_dir(out)) return; }
  snprintf(out, n, "%s", h);
}

static void doc_name(Doc *d, char *out, size_t n) { snprintf(out, n, "%s", d->path[0] ? basename_of(d->path) : "Untitled"); }

static void config_path(char *out, size_t n) {
  const char *x = getenv("XDG_CONFIG_HOME");
  char base[1024];
  if (x && *x) snprintf(base, sizeof base, "%s", x);
  else { home_dir(base, sizeof base, NULL); strncat(base, "/.config", sizeof base - strlen(base) - 1); }
  mkdir(base, 0755);
  strncat(base, "/chlorum", sizeof base - strlen(base) - 1);
  mkdir(base, 0755);
  snprintf(out, n, "%s/typewriter.conf", base);
}

static void save_settings(void) {
  char p[1100];
  config_path(p, sizeof p);
  FILE *f = fopen(p, "w");
  if (!f) return;
  fprintf(f, "style %d\nsize %d\nspacing %d\nmargins %d\nink %d\npaper %d\nui %d\nzoom %d\ncalib %d\ndir %s\n",
          S.style, S.size, S.spacing, S.margin, S.ink, S.paper, S.ui, S.zoom, S.calib, S.dir);
  fprintf(f, "hidden %d\npdfmode %d\npagenum %d\nheader %d\ntz %d\ndatefmt %d\nauthor %s\ntzcustom %s\n", S.hidden, S.pdfmode, S.pagenum, S.header, S.tz, S.datefmt, S.author, S.tzcustom);
  for (int a = 0; a < NACT; a++) {
    int same = 1;
    for (int k = 0; k < 3; k++) {
      Bind d = ACTS[a].def[k] ? bind_parse(ACTS[a].def[k]) : (Bind){0, 0};
      if (d.sc != ACTS[a].b[k].sc || d.mod != ACTS[a].b[k].mod) same = 0;
    }
    if (same) continue; /* записуємо лише змінені */
    char b0[64], b1[64], b2[64];
    bind_str(ACTS[a].b[0], b0, sizeof b0); bind_str(ACTS[a].b[1], b1, sizeof b1); bind_str(ACTS[a].b[2], b2, sizeof b2);
    fprintf(f, "key %s %s|%s|%s\n", ACTS[a].id, b0, b1, b2);
  }
  fclose(f);
}

static void load_settings(void) {
  home_dir(S.dir, sizeof S.dir, "Documents");
  char p[1100], line[1200];
  config_path(p, sizeof p);
  FILE *f = fopen(p, "r");
  if (!f) return;
  while (fgets(line, sizeof line, f)) {
    line[strcspn(line, "\n")] = 0;
    char *v = strchr(line, ' ');
    if (!v) continue;
    *v++ = 0;
    int iv = atoi(v);
#define SET(k, field, max) if (!strcmp(line, k) && iv >= 0 && iv < (max)) S.field = iv;
    SET("style", style, 3) SET("size", size, 4) SET("spacing", spacing, 4) SET("margins", margin, 4)
    SET("ink", ink, 4) SET("paper", paper, 3) SET("ui", ui, 5) SET("zoom", zoom, 301) SET("calib", calib, 401)
    SET("hidden", hidden, 2) SET("pdfmode", pdfmode, 2) SET("pagenum", pagenum, 3) SET("header", header, 3) SET("tz", tz, 32) SET("datefmt", datefmt, 32)
    if (S.calib < 30) S.calib = 100;
#undef SET
    if (!strcmp(line, "author")) snprintf(S.author, sizeof S.author, "%s", v);
    if (!strcmp(line, "tzcustom")) snprintf(S.tzcustom, sizeof S.tzcustom, "%s", v);
    if (!strcmp(line, "key")) {
      char *sp = strchr(v, ' ');
      if (!sp) continue;
      *sp++ = 0;
      for (int a = 0; a < NACT; a++)
        if (!strcmp(ACTS[a].id, v)) {
          for (int k = 0; k < 3; k++) {
            char *bar = strchr(sp, '|');
            if (bar) *bar = 0;
            ACTS[a].b[k] = strcmp(sp, "—") ? bind_parse(sp) : (Bind){0, 0};
            if (!bar) { for (int j = k + 1; j < 3; j++) ACTS[a].b[j] = (Bind){0, 0}; break; }
            sp = bar + 1;
          }
        }
    }
    if (!strcmp(line, "dir") && is_dir(v)) snprintf(S.dir, sizeof S.dir, "%s", v);
  }
  fclose(f);
}


/* ================= коментарі для шапки й номерів сторінок =================
 * ~/.config/mashynka.comments: «розширення ...: початок [кінець]».
 * Для коду шапка й номер сторінки записуються як коментар, щоб надрукований текст
 * лишався правильним кодом. Для решти файлів — просто текст. */
typedef struct { char ext[24]; char pre[16], post[16]; } CRule;
static CRule *crules;
static int ncrules;

static const char *CRULES_DEFAULT =
    "# Comment rules for the page header and page number.\n"
    "# Format: extensions separated by spaces, a colon, comment start [comment end]\n"
    "# Files not listed here are plain text (no comment).\n"
    "c h cpp hpp cc cxx hh cs java js mjs ts jsx tsx go rs swift kt kts scala dart php zig v glsl hlsl proto groovy: //\n"
    "py sh bash zsh fish rb pl pm r yaml yml toml conf ini cfg cmake mk nim jl ex exs cr tcl ps1 dockerfile: #\n"
    "lua sql hs elm ada vhd: --\n"
    "lisp clj cljs el scm rkt: ;;\n"
    "asm s nasm: ;\n"
    "tex sty cls erl m: %\n"
    "vim: \"\n"
    "bat cmd: ::\n"
    "css scss less: /* */\n"
    "html htm xml svg md markdown vue: <!-- -->\n";

static void crules_path(char *out, size_t n) {
  char cp[1100];
  config_path(cp, sizeof cp);
  char *sl = strrchr(cp, '/');
  if (sl) *sl = 0;
  snprintf(out, n, "%s/typewriter.comments", cp);
}

static void crules_load(void) {
  char p[1200], line[1024];
  crules_path(p, sizeof p);
  FILE *f = fopen(p, "r");
  if (!f) {
    f = fopen(p, "w");
    if (f) { fputs(CRULES_DEFAULT, f); fclose(f); }
    f = fopen(p, "r");
    if (!f) return;
  }
  free(crules); crules = NULL; ncrules = 0;
  while (fgets(line, sizeof line, f)) {
    line[strcspn(line, "\n")] = 0;
    if (line[0] == '#' || !line[0]) continue;
    char *colon = strchr(line, ':');
    if (!colon) continue;
    *colon = 0;
    char pre[16] = "", post[16] = "";
    sscanf(colon + 1, " %15s %15s", pre, post);
    for (char *tok = strtok(line, " \t,"); tok; tok = strtok(NULL, " \t,")) {
      crules = realloc(crules, (size_t)(ncrules + 1) * sizeof(CRule));
      CRule *c = &crules[ncrules++];
      snprintf(c->ext, sizeof c->ext, "%s", tok);
      snprintf(c->pre, sizeof c->pre, "%s", pre);
      snprintf(c->post, sizeof c->post, "%s", post);
    }
  }
  fclose(f);
}

static const CRule *crule_for(const char *path) {
  const char *b = strrchr(path, '/');
  b = b ? b + 1 : path;
  const char *dot = strrchr(b, '.');
  const char *ext = dot && dot != b ? dot + 1 : b; /* «Dockerfile» без розширення */
  for (int i = 0; i < ncrules; i++) if (!strcasecmp(crules[i].ext, ext)) return &crules[i];
  return NULL;
}

/* Число з парною кількістю десяткових цифр: 7 → 07, 123 → 0123. */
static void even_num(int v, char *out, size_t n) {
  char t[32];
  snprintf(t, sizeof t, "%d", v);
  snprintf(out, n, "%s%s", strlen(t) % 2 ? "0" : "", t);
}

/* Текст шапки (which=0) або номера сторінки (which=1) для сторінки p. */
static void deco_text(int p, int which, char *out, size_t n) {
  const CRule *c = crule_for(D->path);
  const char *pre = c ? c->pre : "", *post = c ? c->post : "";
  char num[32];
  even_num(p + 1, num, sizeof num);
  if (which == 0) {
    int l0 = p * TPP, l1 = l0 + TPP - 1;
    if (l1 >= D->nL) l1 = D->nL - 1;
    if (l0 >= D->nL) l0 = l1;
    snprintf(out, n, "%spage %s lines %d-%d%s", pre, num, D->L[l0].src + 1, D->L[l1].src + 1, post);
  } else snprintf(out, n, "%s%s%s", pre, num, post);
}

/* ================= розумне форматування =================
 * Працює в звичайному тексті (у коді — ні, щоб нічого не зсувати):
 *  • рядок по центру чи праворуч лишається вирівняним, поки його редагуєш;
 *  • лінія «=====» / «-----» під заголовком завжди такої ж довжини, як заголовок;
 *  • Enter у пункті списку («1. », «- ») починає наступний пункт і перенумеровує список,
 *    Enter у порожньому пункті — закінчує список. */
static int smart_doc(void) {
  if (!D || D->ro || is_media_view(D->view)) return 0;
  const CRule *c = crule_for(D->path);
  if (!c) return 1;
  const char *dot = strrchr(D->path, '.');
  return dot && (!strcasecmp(dot, ".md") || !strcasecmp(dot, ".markdown"));
}

static struct { int on, mode; } SM; /* mode: 1 по центру, 2 праворуч */

static void line_trim(int p, int *ls, int *le, int *ts, int *te) {
  *ls = lstart(p); *le = lend(p);
  *ts = *ls; *te = *le;
  while (*ts < *te && D->c[*ts] == ' ') (*ts)++;
  while (*te > *ts && D->c[*te - 1] == ' ') (*te)--;
}

static void smart_pre(void) {
  SM.on = 0;
  if (!smart_doc() || D->cur != D->anchor) return;
  int ls, le, ts, te;
  line_trim(D->cur, &ls, &le, &ts, &te);
  int len = te - ts, lead = ts - ls;
  if (len <= 0 || lead < 2) return;
  if (abs(lead - (COLS - len) / 2) <= 1) SM.mode = 1;
  else if (te - ls >= COLS - 1) SM.mode = 2;
  else return;
  SM.on = 1;
}

/* Тиха правка без нової точки скасування — продовжує щойно зроблений крок. */
static void quiet_replace(int a, int b, const uint32_t *s, int n) {
  replace_range(a, b, s, n);
  D->ver++;
}

static void smart_post(void) {
  if (!smart_doc()) return;
  int changed = 0, ls, le, ts, te;
  line_trim(D->cur, &ls, &le, &ts, &te);
  int len = te - ts;
  if (SM.on && len > 0) { /* тримаємо вирівнювання */
    int lead = ts - ls, want = SM.mode == 1 ? (COLS - len) / 2 : COLS - len;
    if (want < 0) want = 0;
    if (want != lead) {
      int rel = D->cur - ts;
      uint32_t *sp = malloc((size_t)(want + 1) * sizeof(uint32_t));
      for (int i = 0; i < want; i++) sp[i] = ' ';
      quiet_replace(ls, ts, sp, want);
      free(sp);
      ts = ls + want; te = ts + len;
      D->cur = ts + (rel < 0 ? 0 : rel);
      changed = 1;
    }
  }
  SM.on = 0;
  /* лінія під заголовком — тієї ж довжини й там само, де текст */
  int nls = lend(D->cur);
  if (len > 0 && nls < D->n && !is_rule_line(D->c + ls, te - ls)) {
    int rs = nls + 1, re = lend(rs);
    if (re > rs && is_rule_line(D->c + rs, re - rs)) {
      int k = rs;
      while (k < re && D->c[k] == ' ') k++;
      uint32_t ch = D->c[k];
      int lead = ts - ls, cur_lead = k - rs, cur_len = 0;
      for (int q = k; q < re && D->c[q] == ch; q++) cur_len++;
      if (cur_lead != lead || cur_len != len || re - rs != lead + len) {
        uint32_t *ln = malloc((size_t)(lead + len + 1) * sizeof(uint32_t));
        for (int i = 0; i < lead; i++) ln[i] = ' ';
        for (int i = 0; i < len; i++) ln[lead + i] = ch;
        quiet_replace(rs, re, ln, lead + len);
        free(ln);
        changed = 1;
      }
    }
  }
  if (changed) { D->anchor = D->cur; D->last_cur = D->cur; layout(D); }
}

/* Номер і відступ пункту списку в рядку [ls, le): 1 — «N. », 2 — «- », 3 — «* ». */
static int list_item(int ls, int le, int *ind, int *num, int *mlen) {
  int i = ls;
  while (i < le && D->c[i] == ' ') i++;
  *ind = i - ls;
  if (i + 1 < le && (D->c[i] == '-' || D->c[i] == '*') && D->c[i + 1] == ' ') { *mlen = 2; *num = 0; return D->c[i] == '-' ? 2 : 3; }
  int k = i, v = 0;
  while (k < le && D->c[k] >= '0' && D->c[k] <= '9' && k - i < 6) v = v * 10 + (int)(D->c[k++] - '0');
  if (k > i && k + 1 < le + 1 && k < le && D->c[k] == '.' && (k + 1 == le || D->c[k + 1] == ' ')) { *num = v; *mlen = k - i + (k + 1 < le ? 2 : 1); return 1; }
  return 0;
}

/* Перенумеровує пункти «N. » з тим самим відступом, починаючи з рядка, що містить p. */
static void renumber_from(int p, int ind, int start) {
  int n = start;
  for (int ls = lstart(p); ls <= D->n;) {
    int le = lend(ls), li, num, ml;
    if (list_item(ls, le, &li, &num, &ml) != 1 || li != ind) break;
    if (num != n) {
      char b[16];
      int bn = snprintf(b, sizeof b, "%d. ", n);
      uint32_t u[16];
      for (int i = 0; i < bn; i++) u[i] = (uint32_t)b[i];
      int ms = ls + li, me = ms + ml;
      int rel = D->cur >= me ? D->cur - me : -1;
      quiet_replace(ms, me, u, bn);
      if (rel >= 0 && D->cur >= ms) D->cur = ms + bn + rel;
      le = lend(ls);
    }
    n++;
    if (le >= D->n) break;
    ls = le + 1;
  }
}

/* Enter у списку. 1 — оброблено. */
static int smart_enter(void) {
  if (!smart_doc() || D->cur != D->anchor) return 0;
  int ls = lstart(D->cur), le = lend(D->cur), ind, num, ml;
  int kind = list_item(ls, le, &ind, &num, &ml);
  if (!kind || D->cur < ls + ind + ml) return 0;
  int content = 0;
  for (int i = ls + ind + ml; i < le; i++) if (D->c[i] != ' ') content = 1;
  if (!content) { /* порожній пункт — список закінчено */
    before_edit(0);
    replace_range(ls, le, NULL, 0);
    D->cur = ls;
    after_edit();
    return 1;
  }
  char b[32];
  int bn = kind == 1 ? snprintf(b, sizeof b, "%d. ", num + 1) : snprintf(b, sizeof b, "%c ", kind == 2 ? '-' : '*');
  uint32_t u[64];
  int k = 0;
  u[k++] = '\n';
  for (int i = 0; i < ind && k < 40; i++) u[k++] = ' ';
  for (int i = 0; i < bn; i++) u[k++] = (uint32_t)b[i];
  insert(u, k, 0);
  if (kind == 1 && D->cur < D->n) { /* наступні пункти зсуваються на 1 */
    int nx = lend(D->cur);
    if (nx < D->n) renumber_from(nx + 1, ind, num + 2);
    layout(D);
  }
  D->anchor = D->cur; D->last_cur = D->cur;
  return 1;
}

/* ================= вкладки ================= */
static void tree_rebuild(void);

static void activate(int i) {
  if (i < 0 || i >= ndocs) return;
  act = i;
  D = docs[i];
  ensure_layout(D);
  need_visible = 0;
}

static int new_tab(void) {
  if (ndocs >= MAXDOCS) { set_toast("Too many open tabs"); return act; }
  Doc *d = calloc(1, sizeof(Doc));
  d->desired_col = -1;
  d->last_cur = -1;
  d->uid = ++uid_next;
  reserve(d, 16);
  layout(d);
  int at = ndocs ? act + 1 : 0;
  memmove(docs + at + 1, docs + at, (size_t)(ndocs - at) * sizeof(Doc *));
  docs[at] = d;
  ndocs++;
  activate(at);
  return at;
}

static void free_doc(Doc *d) {
  media_free(d->media);
  clear_history(d);
  free(d->us); free(d->rs); free(d->c); free(d->L);
  free(d);
}

static void close_tab_now(int i) {
  if (i < 0 || i >= ndocs) return;
  free_doc(docs[i]);
  memmove(docs + i, docs + i + 1, (size_t)(ndocs - i - 1) * sizeof(Doc *));
  ndocs--;
  if (!ndocs) { D = NULL; act = 0; return; } /* порожньо — значить порожньо */
  if (act >= ndocs) act = ndocs - 1;
  else if (i < act) act--;
  activate(act);
}

static int find_doc(const char *p) {
  for (int i = 0; i < ndocs; i++) if (!strcmp(docs[i]->path, p)) return i;
  return -1;
}

static int file_type(const char *p, int *enc);
static char *to_utf8(const unsigned char *b, size_t n, int enc, size_t *outn);
static int from_doc(Doc *d, char **out, size_t *outn);

static int load_into(Doc *d, const char *p) {
  FILE *f = fopen(p, "rb");
  d->n = 0;
  media_free(d->media);
  d->media = NULL;
  d->enc = ENC_UTF8; d->crlf = 0; d->bom = 0; d->ftype = FT_TEXT;
  if (f) {
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    char *raw = malloc(n + 1);
    size_t got = fread(raw, 1, n, f);
    fclose(f);
    int enc = ENC_UTF8;
    file_type(p, &enc);
    d->enc = enc;
    d->bom = enc == ENC_UTF8 && got >= 3 && !memcmp(raw, "\xEF\xBB\xBF", 3);
    size_t un;
    char *buf = to_utf8((unsigned char *)raw, got, enc, &un);
    free(raw);
    size_t crlf = 0, lf = 0;
    for (size_t i = 0; i < un; i++) if (buf[i] == '\n') { if (i && buf[i - 1] == '\r') crlf++; else lf++; }
    d->crlf = crlf > lf; /* зберігаємо так само, як було у файлі */
    int m;
    uint32_t *cp = utf8_dec(buf, un, &m);
    free(buf);
    reserve(d, m);
    memcpy(d->c, cp, (size_t)m * sizeof(uint32_t));
    d->n = m;
    free(cp);
  }
  snprintf(d->path, sizeof d->path, "%s", p);
  d->cur = d->anchor = 0;
  d->dirty = 0;
  d->ro = 0;
  d->view = VIEW_TEXT;
  d->scroll = 0;
  d->ver++;
  clear_history(d);
  layout(d);
  return f != NULL;
}

/* Відкриває файл у вкладці: якщо вже відкритий — переходить до нього. */
static int load_view(Doc *d, const char *p, int view);

static void open_file(const char *p) {
  int i = find_doc(p);
  if (i >= 0) { activate(i); return; }
  int enc = ENC_UTF8, ft = file_type(p, &enc);
  if (!(D && !D->path[0] && !D->dirty && D->n == 0 && !D->media)) new_tab();
  if (ft == FT_TEXT) {
    load_into(D, p);
    if (D->enc != ENC_UTF8 || D->crlf) {
      char m[200];
      snprintf(m, sizeof m, "Encoding %s%s: it is saved the same way", ENC_NAME[D->enc], D->crlf ? ", CRLF lines (Windows)" : "");
      set_toast(m);
    }
    return;
  }
  D->ftype = ft;
  int want = ft == FT_PDF ? VIEW_PDF : ft == FT_IMAGE ? VIEW_IMAGE : ft == FT_ELF ? VIEW_ASM :
             (ft == FT_ARCHIVE || ft == FT_DOCX || ft == FT_ODT || ft == FT_EPUB || ft == FT_MEDIA) ? VIEW_EXTRACT : VIEW_HEX;
  int v = load_view(D, p, want);
  static const char *what[] = {"", "PDF", "Image", "Archive: contents", "Word document: text", "ODT document: text", "EPUB book: text",
                               "Audio/video: properties", "Program: assembly", "Binary file: hex"};
  char m[200];
  snprintf(m, sizeof m, "%s · view only%s", v == VIEW_HEX && ft != FT_BINARY ? "Showing hex" : what[ft], ft != FT_BINARY ? " · Ctrl+B: another view" : "");
  set_toast(m);
}

/* Слайдшоу з теки: випадкова картинка, нова кожні 5 секунд. */
static void open_slides(const char *dir) {
  for (int i = 0; i < ndocs; i++) if (docs[i]->view == VIEW_SLIDES && !strcmp(docs[i]->path, dir)) { activate(i); return; }
  if (!(D && !D->path[0] && !D->dirty && D->n == 0 && !D->media)) new_tab();
  load_view(D, dir, VIEW_SLIDES);
  set_toast("Slideshow: a random picture every 5 s · → next, ← back, Space pauses");
}

static int save_to(Doc *d, const char *p) {
  if (d->ro && !strcmp(p, d->path)) { set_toast("This file is open for viewing only: save the text as a new file"); return 0; }
  if (is_media_view(d->view)) { set_toast("Pictures and PDFs can only be viewed here"); return 0; }
  if (d->ro) { d->enc = ENC_UTF8; d->crlf = 0; d->bom = 0; } /* текст із перегляду зберігаємо як звичайний UTF-8 */
  size_t n;
  char *s;
  int bad = from_doc(d, &s, &n);
  FILE *f = fopen(p, "wb");
  int ok = f && fwrite(s, 1, n, f) == n;
  if (f) fclose(f);
  free(s);
  char msg[600];
  if (ok) {
    if (p != d->path) snprintf(d->path, sizeof d->path, "%s", p);
    d->dirty = 0;
    d->ro = 0;
    d->view = VIEW_TEXT;
    d->ftype = FT_TEXT;
    dirname_of(p, S.dir, sizeof S.dir);
    save_settings();
    if (bad) snprintf(msg, sizeof msg, "Saved; %d characters do not exist in %s and became “?”", bad, ENC_NAME[d->enc]);
    else snprintf(msg, sizeof msg, "Saved");
    { char cp[1200]; crules_path(cp, sizeof cp); if (!strcmp(cp, d->path)) { crules_load(); snprintf(msg, sizeof msg, "Comment rules updated"); } }
    tree_rebuild();
  } else snprintf(msg, sizeof msg, "Could not save %s", basename_of(p));
  set_toast(msg);
  return ok;
}

/* ================= шрифти ================= */
typedef struct {
  FT_Face face;
  hb_blob_t *blob;
  hb_face_t *hface;
  hb_font_t *hb;
  unsigned char *data;
  long len;
  int ng, upem;
} Font;
static Font fdoc, fui, fuib, fmono;
static int doc_adv, have_doc_font;
static char doc_psname[128];

typedef struct { SDL_Texture *t; int w, h, bx, by; } Glyph;
typedef struct { Font *f; long px64; int n; Glyph *g; } GCache;
static GCache c_doc = {&fdoc, 0, 0, 0}, c_ui = {&fui, 0, 0, 0}, c_uib = {&fuib, 0, 0, 0},
              c_small = {&fui, 0, 0, 0}, c_term = {&fmono, 0, 0, 0};

static int font_load(Font *f, const char *fp, int index) {
  FILE *fl = fopen(fp, "rb");
  if (!fl) return 0;
  fseek(fl, 0, SEEK_END);
  long n = ftell(fl);
  fseek(fl, 0, SEEK_SET);
  unsigned char *data = malloc(n);
  if (fread(data, 1, n, fl) != (size_t)n) { fclose(fl); free(data); return 0; }
  fclose(fl);
  FT_Face face;
  if (FT_New_Memory_Face(ftl, data, n, index, &face)) { free(data); return 0; }
  f->face = face; f->data = data; f->len = n;
  f->blob = hb_blob_create((const char *)data, n, HB_MEMORY_MODE_READONLY, NULL, NULL);
  f->hface = hb_face_create(f->blob, index);
  f->hb = hb_font_create(f->hface);
  f->upem = face->units_per_EM;
  hb_font_set_scale(f->hb, f->upem, f->upem);
  f->ng = face->num_glyphs;
  return 1;
}

static void font_free(Font *f) {
  if (!f->face) return;
  hb_font_destroy(f->hb); hb_face_destroy(f->hface); hb_blob_destroy(f->blob);
  FT_Done_Face(f->face);
  free(f->data);
  memset(f, 0, sizeof *f);
}

static void gcache_clear(GCache *c) {
  if (c->g) { for (int i = 0; i < c->n; i++) if (c->g[i].t) SDL_DestroyTexture(c->g[i].t); free(c->g); }
  c->g = NULL; c->n = 0; c->px64 = 0;
}

static void gcache_size(GCache *c, double px) {
  long p64 = lround(px * 64);
  if (p64 < 64) p64 = 64;
  if (c->px64 != p64) { gcache_clear(c); c->px64 = p64; }
}

static void recompute_metrics(void) {
  FONT_PT = SIZES[S.size];
  LEAD_PT = FONT_PT * 7.0 / 6.0 * SPACING[S.spacing];
  MARGIN_X = MARGIN_TOP = MARGIN_BOT = MARGINS[S.margin] * MM;
  if (have_doc_font) {
    ADVPT = doc_adv * FONT_PT / fdoc.upem;
    double asc = fdoc.face->ascender * FONT_PT / fdoc.upem, desc = -fdoc.face->descender * FONT_PT / fdoc.upem;
    BASEPT = (FONT_PT * 7.0 / 6.0 - (asc + desc)) / 2 + asc;
  } else { ADVPT = FONT_PT * 0.6; BASEPT = FONT_PT; }
  COLS = (int)floor((PAGE_W - 2 * MARGIN_X) / ADVPT + 1e-6);
  LPP = (int)floor((PAGE_H - MARGIN_TOP - MARGIN_BOT) / LEAD_PT + 1e-6);
  if (LPP < 1) LPP = 1;
  HDR = S.header ? 1 : 0;
  FTR = S.pagenum ? 1 : 0;
  if (LPP - HDR - FTR < 1) { HDR = FTR = 0; }
  TPP = LPP - HDR - FTR;
  g_lgen++;
  if (D) ensure_layout(D);
  need_visible = 1;
}

static int find_doc_font(const char *name, char *out, size_t n) {
  char *bp = SDL_GetBasePath();
  const char *base = bp ? bp : "./";
  const char *fmt[] = {"%sfonts/%s.ttf", "%s%s.ttf", "%s../fonts/%s.ttf", "%s../share/mashynka/fonts/%s.ttf"};
  int ok = 0;
  for (int i = 0; i < 4 && !ok; i++) {
    snprintf(out, n, fmt[i], base, name);
    FILE *f = fopen(out, "rb");
    if (f) { fclose(f); ok = 1; }
  }
  if (!ok) { snprintf(out, n, "fonts/%s.ttf", name); FILE *f = fopen(out, "rb"); if (f) { fclose(f); ok = 1; } }
  SDL_free(bp);
  return ok;
}

static int load_doc_font(int st) {
  char fp[1400];
  if (!find_doc_font(STYLE_FILE[st], fp, sizeof fp)) return 0;
  Font nf = {0};
  if (!font_load(&nf, fp, 0)) return 0;
  gcache_clear(&c_doc);
  font_free(&fdoc);
  fdoc = nf;
  hb_codepoint_t gm = 0;
  hb_font_get_nominal_glyph(fdoc.hb, 'M', &gm);
  doc_adv = hb_font_get_glyph_h_advance(fdoc.hb, gm);
  const char *pn = FT_Get_Postscript_Name(fdoc.face);
  snprintf(doc_psname, sizeof doc_psname, "%s", pn ? pn : STYLE_FILE[st]);
  have_doc_font = 1;
  S.style = st;
  recompute_metrics();
  return 1;
}

static int fc_find(const char *pattern, char *out, size_t n, int *index) {
  FcPattern *p = FcNameParse((const FcChar8 *)pattern);
  if (!p) return 0;
  FcConfigSubstitute(NULL, p, FcMatchPattern);
  FcDefaultSubstitute(p);
  FcResult r;
  FcPattern *m = FcFontMatch(NULL, p, &r);
  int ok = 0;
  if (m) {
    FcChar8 *file;
    if (FcPatternGetString(m, FC_FILE, 0, &file) == FcResultMatch) { snprintf(out, n, "%s", (char *)file); ok = 1; }
    *index = 0;
    FcPatternGetInteger(m, FC_INDEX, 0, index);
    FcPatternDestroy(m);
  }
  FcPatternDestroy(p);
  return ok;
}

static void load_ui_fonts(void) {
  char fp[1400];
  int idx;
  FcInit();
  if (fc_find("sans-serif:lang=uk", fp, sizeof fp, &idx)) font_load(&fui, fp, idx);
  if (fc_find("sans-serif:weight=medium:lang=uk", fp, sizeof fp, &idx)) font_load(&fuib, fp, idx);
  if (fc_find("monospace:lang=uk", fp, sizeof fp, &idx)) font_load(&fmono, fp, idx);
  if (!fui.face && fdoc.face) { char dp[1400]; if (find_doc_font(STYLE_FILE[0], dp, sizeof dp)) font_load(&fui, dp, 0); }
  if (!fuib.face) c_uib.f = &fui;
  if (!fmono.face) c_term.f = &fui;
}

static unsigned shape(Font *f, const uint32_t *cp, int n, hb_glyph_info_t **inf, hb_glyph_position_t **pos) {
  hb_buffer_clear_contents(hbbuf);
  hb_buffer_add_utf32(hbbuf, cp, n, 0, n);
  hb_buffer_set_direction(hbbuf, HB_DIRECTION_LTR);
  hb_buffer_guess_segment_properties(hbbuf);
  hb_shape(f->hb, hbbuf, NULL, 0);
  unsigned len;
  *inf = hb_buffer_get_glyph_infos(hbbuf, &len);
  if (pos) *pos = hb_buffer_get_glyph_positions(hbbuf, &len);
  return len;
}

static Glyph *get_glyph(GCache *c, unsigned gid) {
  Font *f = c->f;
  if (!f->face) return NULL;
  if (!c->g) { c->n = f->ng; c->g = calloc(f->ng, sizeof(Glyph)); }
  if ((int)gid >= c->n) return NULL;
  Glyph *g = &c->g[gid];
  if (g->t || g->w < 0) return g;
  FT_Set_Char_Size(f->face, 0, c->px64, 72, 72);
  if (FT_Load_Glyph(f->face, gid, FT_LOAD_RENDER | FT_LOAD_TARGET_LIGHT)) { g->w = -1; return g; }
  FT_Bitmap *bm = &f->face->glyph->bitmap;
  if (!bm->width || !bm->rows || bm->pixel_mode != FT_PIXEL_MODE_GRAY) { g->w = -1; return g; }
  uint32_t *px = malloc(bm->width * bm->rows * 4);
  for (unsigned y = 0; y < bm->rows; y++)
    for (unsigned x = 0; x < bm->width; x++)
      px[y * bm->width + x] = ((uint32_t)bm->buffer[y * bm->pitch + x] << 24) | 0xFFFFFF;
  g->t = SDL_CreateTexture(R, SDL_PIXELFORMAT_ARGB8888, SDL_TEXTUREACCESS_STATIC, bm->width, bm->rows);
  SDL_UpdateTexture(g->t, NULL, px, bm->width * 4);
  SDL_SetTextureBlendMode(g->t, SDL_BLENDMODE_BLEND);
  free(px);
  g->w = bm->width; g->h = bm->rows;
  g->bx = f->face->glyph->bitmap_left; g->by = f->face->glyph->bitmap_top;
  return g;
}

static void blit_glyph(Glyph *gl, float x, float base, RGB col, Uint8 a) {
  if (!gl || gl->w <= 0) return;
  SDL_SetTextureColorMod(gl->t, col.r, col.g, col.b);
  SDL_SetTextureAlphaMod(gl->t, a);
  SDL_Rect d = {(int)lroundf(x) + gl->bx, (int)lroundf(base) - gl->by, gl->w, gl->h};
  SDL_RenderCopy(R, gl->t, NULL, &d);
}

static void draw_mono(const uint32_t *cp, int n, float x0, float base, float adv, RGB col) {
  if (n <= 0 || !have_doc_font) return;
  hb_glyph_info_t *inf;
  unsigned g = shape(&fdoc, cp, n, &inf, NULL);
  for (unsigned i = 0; i < g; i++) blit_glyph(get_glyph(&c_doc, inf[i].codepoint), x0 + inf[i].cluster * adv, base, col, 255);
}

static float measure(GCache *c, const char *s) {
  if (!c->f->face || !*s) return 0;
  int n;
  uint32_t *cp = utf8_dec(s, strlen(s), &n);
  hb_glyph_info_t *inf;
  hb_glyph_position_t *pos;
  unsigned g = shape(c->f, cp, n, &inf, &pos);
  double w = 0;
  for (unsigned i = 0; i < g; i++) w += pos[i].x_advance;
  free(cp);
  return (float)(w * c->px64 / 64.0 / c->f->upem);
}

/* align: 0 ліворуч, 1 по центру, 2 праворуч. */
static float text(GCache *c, const char *s, float x, float base, RGB col, int align) {
  if (!c->f->face || !*s) return 0;
  float w = measure(c, s);
  if (align == 1) x -= w / 2;
  else if (align == 2) x -= w;
  int n;
  uint32_t *cp = utf8_dec(s, strlen(s), &n);
  hb_glyph_info_t *inf;
  hb_glyph_position_t *pos;
  unsigned g = shape(c->f, cp, n, &inf, &pos);
  double sc = c->px64 / 64.0 / c->f->upem, pen = 0;
  for (unsigned i = 0; i < g; i++) {
    blit_glyph(get_glyph(c, inf[i].codepoint), (float)(x + (pen + pos[i].x_offset) * sc), base, col, 255);
    pen += pos[i].x_advance;
  }
  free(cp);
  return w;
}

static float mid(GCache *c, float y, float h) { return y + h / 2 + (float)(c->px64 / 64.0) * 0.36f; }


static void fit_right(GCache *c, const char *s, float maxw, char *out, size_t n) {
  snprintf(out, n, "%s", s);
  if (measure(c, out) <= maxw) return;
  size_t len = strlen(s);
  while (len > 0) {
    len--;
    while (len > 0 && (s[len] & 0xC0) == 0x80) len--;
    snprintf(out, n, "%.*s…", (int)len, s);
    if (measure(c, out) <= maxw) return;
  }
}

/* ================= примітиви ================= */
static void fill_rect(float x, float y, float w, float h, RGB c, Uint8 a) {
  SDL_SetRenderDrawColor(R, c.r, c.g, c.b, a);
  SDL_FRect r = {x, y, w, h};
  SDL_RenderFillRectF(R, &r);
}

static void fill_rrect(float x, float y, float w, float h, float r, RGB c, Uint8 a) {
  if (w <= 0 || h <= 0) return;
  if (r > w / 2) r = w / 2;
  if (r > h / 2) r = h / 2;
  if (r < 1) { fill_rect(x, y, w, h, c, a); return; }
  enum { SEG = 6, NP = 4 * (SEG + 1) };
  SDL_Vertex v[NP + 1];
  int idx[NP * 3];
  SDL_Color col = {c.r, c.g, c.b, a};
  float cx[4] = {x + w - r, x + w - r, x + r, x + r}, cy[4] = {y + r, y + h - r, y + h - r, y + r};
  v[0].position = (SDL_FPoint){x + w / 2, y + h / 2};
  v[0].color = col;
  v[0].tex_coord = (SDL_FPoint){0, 0};
  int k = 1;
  for (int q = 0; q < 4; q++)
    for (int s = 0; s <= SEG; s++) {
      double ang = -M_PI / 2 + q * M_PI / 2 + s * (M_PI / 2) / SEG;
      v[k].position = (SDL_FPoint){cx[q] + r * (float)cos(ang), cy[q] + r * (float)sin(ang)};
      v[k].color = col;
      v[k].tex_coord = (SDL_FPoint){0, 0};
      k++;
    }
  for (int i = 0; i < NP; i++) { idx[i * 3] = 0; idx[i * 3 + 1] = 1 + i; idx[i * 3 + 2] = 1 + (i + 1) % NP; }
  SDL_RenderGeometry(R, NULL, v, NP + 1, idx, NP * 3);
}

static float hair(void) { return fmaxf(1, floorf((float)u)); }

static void box(float x, float y, float w, float h, float r, RGB fill, RGB border) {
  float b = hair();
  fill_rrect(x, y, w, h, r, border, 255);
  fill_rrect(x + b, y + b, w - 2 * b, h - 2 * b, r - b, fill, 255);
}

static void clip(float x, float y, float w, float h) {
  SDL_Rect r = {(int)x, (int)y, (int)ceilf(w), (int)ceilf(h)};
  SDL_RenderSetClipRect(R, &r);
}

/* Хрестик (закрити) з двох тонких ліній. */
static void draw_x(float cx, float cy, float s, RGB c) {
  float t = fmaxf(1.2f, 1.3f * (float)u) / 2;
  for (int k = 0; k < 2; k++) {
    float dx = s / 2, dy = k ? -s / 2 : s / 2;
    float nx = -dy, ny = dx, l = sqrtf(nx * nx + ny * ny);
    nx = nx / l * t; ny = ny / l * t;
    SDL_Color col = {c.r, c.g, c.b, 255};
    SDL_Vertex v[4] = {{{cx - dx + nx, cy - dy + ny}, col, {0, 0}}, {{cx + dx + nx, cy + dy + ny}, col, {0, 0}},
                       {{cx + dx - nx, cy + dy - ny}, col, {0, 0}}, {{cx - dx - nx, cy - dy - ny}, col, {0, 0}}};
    int idx[6] = {0, 1, 2, 0, 2, 3};
    SDL_RenderGeometry(R, NULL, v, 4, idx, 6);
  }
}

static void draw_tri(float x, float y, float s, int dir, RGB c) {
  SDL_Color col = {c.r, c.g, c.b, 255};
  SDL_Vertex v[3];
  if (dir == 0) { v[0].position = (SDL_FPoint){x + s * .3f, y}; v[1].position = (SDL_FPoint){x + s * .75f, y + s / 2}; v[2].position = (SDL_FPoint){x + s * .3f, y + s}; }
  else if (dir == 1) { v[0].position = (SDL_FPoint){x, y + s * .3f}; v[1].position = (SDL_FPoint){x + s, y + s * .3f}; v[2].position = (SDL_FPoint){x + s / 2, y + s * .75f}; }
  else if (dir == 2) { v[0].position = (SDL_FPoint){x, y + s * .7f}; v[1].position = (SDL_FPoint){x + s, y + s * .7f}; v[2].position = (SDL_FPoint){x + s / 2, y + s * .25f}; }
  else { v[0].position = (SDL_FPoint){x, y + s * .3f}; v[1].position = (SDL_FPoint){x + s, y + s * .3f}; v[2].position = (SDL_FPoint){x + s / 2, y + s * .75f}; }
  for (int i = 0; i < 3; i++) { v[i].color = col; v[i].tex_coord = (SDL_FPoint){0, 0}; }
  SDL_RenderGeometry(R, NULL, v, 3, NULL, 0);
}

/* ================= віджети ================= */
static struct { float x, y; int down, pressed, released, clicks, mods; } M;
static int layer, top_layer, active_id;

static int inside(float x, float y, float w, float h) { return M.x >= x && M.x < x + w && M.y >= y && M.y < y + h; }

static int ic_on; /* обмеження області, де віджети реагують на мишку (прокручуваний вміст) */
static float ic_x, ic_y, ic_w, ic_h;
static int interact(int id, float x, float y, float w, float h, int *hov) {
  int in = layer == top_layer && inside(x, y, w, h) && (!ic_on || inside(ic_x, ic_y, ic_w, ic_h));
  if (hov) *hov = in;
  if (in && M.pressed) active_id = id;
  return in && M.released && active_id == id;
}

enum { BT_NORMAL, BT_PRIMARY, BT_GHOST, BT_ON };
static float btn_w(const char *label) { return measure(&c_ui, label) + 24 * (float)u; }

static int button(int id, float x, float y, float w, float h, const char *label, int kind, int enabled) {
  int hov = 0, clk = enabled ? interact(id, x, y, w, h, &hov) : 0;
  float r = 6 * (float)u;
  RGB txt = C_TXT;
  if (kind == BT_PRIMARY) { fill_rrect(x, y, w, h, r, hov ? (RGB){236, 190, 122} : C_ACC, 255); txt = C_ACC_DARK; }
  else if (kind == BT_ON) { fill_rrect(x, y, w, h, r, C_HOVER, 255); txt = C_ACC; }
  else if (kind == BT_GHOST) { if (hov) fill_rrect(x, y, w, h, r, C_HOVER, 255); }
  else box(x, y, w, h, r, hov ? C_HOVER : C_PANEL2, C_LINE);
  if (!enabled) txt = C_FAINT;
  text(&c_ui, label, x + w / 2, mid(&c_ui, y, h), txt, 1);
  return clk;
}

/* Однорядкове поле введення. */
typedef struct { char s[512]; int caret, all; } Field; /* all: усе виділено, друк замінює */
static Field *ffocus;
static Uint32 blink_t;

static void field_set(Field *f, const char *s) { snprintf(f->s, sizeof f->s, "%s", s); f->caret = (int)strlen(f->s); }

static void field_text(Field *f, const char *t) {
  if (f->all) { f->s[0] = 0; f->caret = 0; f->all = 0; }
  size_t n = strlen(f->s), k = strlen(t);
  if (n + k >= sizeof f->s - 1) return;
  memmove(f->s + f->caret + k, f->s + f->caret, n - f->caret + 1);
  memcpy(f->s + f->caret, t, k);
  f->caret += (int)k;
}

static int u8prev(const char *s, int i) { if (i <= 0) return 0; i--; while (i > 0 && (s[i] & 0xC0) == 0x80) i--; return i; }
static int u8next(const char *s, int i) { if (!s[i]) return i; i++; while (s[i] && (s[i] & 0xC0) == 0x80) i++; return i; }

/* Повертає 1, якщо клавішу оброблено полем. */
static int field_key(Field *f, SDL_Keysym ks) {
  if (f->all && (ks.sym == SDLK_BACKSPACE || ks.sym == SDLK_DELETE)) { f->s[0] = 0; f->caret = 0; f->all = 0; return 1; }
  if (ks.sym != SDLK_LCTRL && ks.sym != SDLK_RCTRL && ks.sym != SDLK_LSHIFT && ks.sym != SDLK_RSHIFT && !(ks.sym == SDLK_v && (ks.mod & CMDMOD))) f->all = 0;
  int n = (int)strlen(f->s);
  switch (ks.sym) {
    case SDLK_LEFT: f->caret = u8prev(f->s, f->caret); return 1;
    case SDLK_RIGHT: f->caret = u8next(f->s, f->caret); return 1;
    case SDLK_HOME: f->caret = 0; return 1;
    case SDLK_END: f->caret = n; return 1;
    case SDLK_BACKSPACE: {
      if (!f->caret) return 1;
      int p = (ks.mod & CMDMOD) ? 0 : u8prev(f->s, f->caret);
      memmove(f->s + p, f->s + f->caret, n - f->caret + 1);
      f->caret = p;
      return 1;
    }
    case SDLK_DELETE: {
      int q = u8next(f->s, f->caret);
      memmove(f->s + f->caret, f->s + q, n - q + 1);
      return 1;
    }
    case SDLK_v:
      if (ks.mod & CMDMOD) {
        if (SDL_HasClipboardText()) {
          char *c = SDL_GetClipboardText();
          for (char *p = c; *p; p++) if (*p == '\n' || *p == '\r' || *p == '\t') *p = ' ';
          field_text(f, c);
          SDL_free(c);
        }
        return 1;
      }
      return 0;
    case SDLK_a: if (ks.mod & CMDMOD) { f->all = 1; f->caret = n; return 1; } return 0;
    default: return 0;
  }
}

static void draw_field(int id, Field *f, float x, float y, float w, float h, const char *ph) {
  int hov;
  if (interact(id, x, y, w, h, &hov) || (hov && M.pressed)) { ffocus = f; blink_t = SDL_GetTicks(); }
  int foc = ffocus == f;
  box(x, y, w, h, 6 * (float)u, C_FIELD, foc ? C_ACC : C_LINE);
  float pad = 10 * (float)u;
  SDL_Rect oldc;
  int had_clip = SDL_RenderIsClipEnabled(R);
  SDL_RenderGetClipRect(R, &oldc);
  clip(x + 2, y, w - 4, h);
  if (had_clip) { /* не виходимо за межі прокручуваного вмісту */
    SDL_Rect nc, cur;
    SDL_RenderGetClipRect(R, &cur);
    if (SDL_IntersectRect(&cur, &oldc, &nc)) SDL_RenderSetClipRect(R, &nc); else { SDL_Rect z = {0, 0, 0, 0}; SDL_RenderSetClipRect(R, &z); }
  }
  if (!f->s[0] && !foc) text(&c_ui, ph, x + pad, mid(&c_ui, y, h), C_FAINT, 0);
  else if (!f->s[0]) text(&c_ui, ph, x + pad, mid(&c_ui, y, h), C_FAINT, 0);
  char pre[512];
  snprintf(pre, sizeof pre, "%.*s", f->caret, f->s);
  float cw = measure(&c_ui, pre), shift = 0;
  if (cw > w - 2 * pad) shift = cw - (w - 2 * pad);
  if (f->all && f->s[0]) fill_rrect(x + pad - shift - 2, y + 6 * (float)u, measure(&c_ui, f->s) + 4, h - 12 * (float)u, 3 * (float)u, (RGB){92, 74, 46}, 255);
  if (f->s[0]) text(&c_ui, f->s, x + pad - shift, mid(&c_ui, y, h), C_TXT, 0);
  if (foc && ((SDL_GetTicks() - blink_t) / 530) % 2 == 0) fill_rect(x + pad + cw - shift, y + 7 * (float)u, fmaxf(1, 1.5f * (float)u), h - 14 * (float)u, C_ACC, 255);
  SDL_RenderSetClipRect(R, had_clip ? &oldc : NULL);
}

/* ================= пошук і заміна ================= */
static struct {
  int open, replace, icase;
  Field q, r;
  int *m, nm, capm, idx, qn;
  Doc *doc;
  unsigned ver;
  char lastq[512];
  int last_icase;
} F = {.icase = 1};

static uint32_t lower(uint32_t c) {
  if (c >= 'A' && c <= 'Z') return c + 32;
  if (c >= 0x410 && c <= 0x42F) return c + 0x20;
  if (c >= 0x400 && c <= 0x40F) return c + 0x50;
  if (c == 0x490) return 0x491;
  return c;
}

static void find_update(void) {
  if (F.doc == D && F.ver == D->ver && !strcmp(F.lastq, F.q.s) && F.last_icase == F.icase) return;
  F.doc = D; F.ver = D->ver; F.last_icase = F.icase;
  snprintf(F.lastq, sizeof F.lastq, "%s", F.q.s);
  F.nm = 0;
  int qn;
  uint32_t *q = utf8_dec(F.q.s, strlen(F.q.s), &qn);
  F.qn = qn;
  if (qn) {
    if (F.icase) for (int i = 0; i < qn; i++) q[i] = lower(q[i]);
    for (int i = 0; i + qn <= D->n;) {
      int k = 0;
      if (F.icase) { while (k < qn && lower(D->c[i + k]) == q[k]) k++; }
      else { while (k < qn && D->c[i + k] == q[k]) k++; }
      if (k == qn) {
        if (F.nm == F.capm) { F.capm = F.capm ? F.capm * 2 : 256; F.m = realloc(F.m, (size_t)F.capm * sizeof(int)); }
        F.m[F.nm++] = i;
        i += qn;
      } else i++;
    }
  }
  free(q);
  if (F.idx >= F.nm) F.idx = F.nm ? F.nm - 1 : 0;
}

static int lower_bound(const int *a, int n, int v) {
  int lo = 0, hi = n;
  while (lo < hi) { int m = (lo + hi) / 2; if (a[m] < v) lo = m + 1; else hi = m; }
  return lo;
}

static void find_select(int k) {
  if (!F.nm) return;
  F.idx = (k % F.nm + F.nm) % F.nm;
  D->anchor = F.m[F.idx];
  D->cur = F.m[F.idx] + F.qn;
  D->desired_col = -1;
  need_visible = 1;
}

static void find_step(int dir) {
  find_update();
  if (!F.nm) return;
  if (dir > 0) {
    int k = lower_bound(F.m, F.nm, sel_a() + (sel_a() == sel_b() ? 0 : 1));
    find_select(k >= F.nm ? 0 : k);
  } else {
    int k = lower_bound(F.m, F.nm, sel_a()) - 1;
    find_select(k < 0 ? F.nm - 1 : k);
  }
}

static void replace_one(void) {
  if (ro_block()) return;
  find_update();
  if (!F.nm) return;
  int a = sel_a(), b = sel_b();
  int k = lower_bound(F.m, F.nm, a);
  if (k < F.nm && F.m[k] == a && b - a == F.qn) {
    int rn;
    uint32_t *r = utf8_dec(F.r.s, strlen(F.r.s), &rn);
    before_edit(0);
    replace_range(a, b, r, rn);
    free(r);
    D->cur = a + rn;
    after_edit();
    find_update();
    int nk = lower_bound(F.m, F.nm, a + rn);
    if (F.nm) find_select(nk >= F.nm ? 0 : nk);
  } else find_step(1);
}

static void replace_all(void) {
  if (ro_block()) return;
  find_update();
  if (!F.nm) { set_toast("Nothing found"); return; }
  int rn;
  uint32_t *r = utf8_dec(F.r.s, strlen(F.r.s), &rn);
  long nn = (long)D->n + (long)F.nm * (rn - F.qn);
  uint32_t *o = malloc((size_t)(nn + 1) * sizeof(uint32_t));
  long k = 0;
  int last = 0, cnt = F.nm;
  for (int i = 0; i < F.nm; i++) {
    memcpy(o + k, D->c + last, (size_t)(F.m[i] - last) * sizeof(uint32_t));
    k += F.m[i] - last;
    memcpy(o + k, r, (size_t)rn * sizeof(uint32_t));
    k += rn;
    last = F.m[i] + F.qn;
  }
  memcpy(o + k, D->c + last, (size_t)(D->n - last) * sizeof(uint32_t));
  k += D->n - last;
  free(r);
  before_edit(0);
  free(D->c);
  D->c = o;
  D->n = (int)k;
  D->cap = (int)nn + 1;
  if (D->cur > D->n) D->cur = D->n;
  after_edit();
  char m[64];
  snprintf(m, sizeof m, "Replaced: %d", cnt);
  set_toast(m);
}

static void find_open(int replace) {
  F.open = 1;
  if (D && D->cur != D->anchor && sel_b() - sel_a() < 200) {
    int a = sel_a(), b = sel_b(), nl = 0;
    for (int i = a; i < b; i++) if (D->c[i] == '\n') nl = 1;
    if (!nl) { char *s = text_utf8(D->c, a, b, NULL); field_set(&F.q, s); free(s); }
  }
  Field *f = replace && F.q.s[0] ? &F.r : &F.q;
  f->caret = (int)strlen(f->s);
  f->all = f->s[0] != 0;
  ffocus = f;
  blink_t = SDL_GetTicks();
}
/* ================= стара папір: процедурна текстура =================
 * Генерується один раз у фоновому потоці (кілька варіантів, щоб сторінки відрізнялися):
 * пожовтіння плямами, потемнілі нерівні краї, іржаві цятки (foxing), слід від чашки,
 * згин посередині, волокна паперу й дрібне зерно. */
#define PT_W 1240
#define PT_H 1754
#define NPT 3
#define PZ_W 827  /* для PDF — 100 dpi: на друці не відрізнити, а файл утричі менший */
#define PZ_H 1169
static unsigned char *paper_rgb[NPT];  /* RGB, PT_W×PT_H */
static unsigned char *paper_z[NPT];    /* те саме, вже стиснуте для PDF — щоб експорт був миттєвим */
static unsigned long paper_zn[NPT];
static SDL_Texture *paper_tex[NPT];
static SDL_atomic_t paper_ready;       /* скільки варіантів уже готово (усі = NPT) */
static SDL_atomic_t paper_var_ok[NPT];
static int paper_started;

static uint32_t hash2(int x, int y, uint32_t s) {
  uint32_t h = (uint32_t)x * 374761393u + (uint32_t)y * 668265263u + s * 2246822519u;
  h = (h ^ (h >> 13)) * 1274126177u;
  return h ^ (h >> 16);
}
static float h01(int x, int y, uint32_t s) { return (hash2(x, y, s) & 0xFFFFFF) / 16777215.0f; }
static float vnoise(float x, float y, uint32_t s) {
  int xi = (int)floorf(x), yi = (int)floorf(y);
  float fx = x - xi, fy = y - yi;
  fx = fx * fx * (3 - 2 * fx); fy = fy * fy * (3 - 2 * fy);
  float a = h01(xi, yi, s), b = h01(xi + 1, yi, s), c = h01(xi, yi + 1, s), d = h01(xi + 1, yi + 1, s);
  return (a + (b - a) * fx) + ((c + (d - c) * fx) - (a + (b - a) * fx)) * fy;
}
static float fbm(float x, float y, int oct, uint32_t s) {
  float v = 0, amp = 0.5f, sum = 0;
  for (int i = 0; i < oct; i++) { v += vnoise(x, y, s + i * 101) * amp; sum += amp; x *= 2.03f; y *= 2.03f; amp *= 0.5f; }
  return v / sum;
}
static float rnd(uint32_t *st) { *st = *st * 1664525u + 1013904223u; return (*st >> 8) / 16777216.0f; }
static float smooth01(float a, float b, float x) { float t = (x - a) / (b - a); t = t < 0 ? 0 : t > 1 ? 1 : t; return t * t * (3 - 2 * t); }

static void gen_paper(int var, unsigned char *out) {
  const int W = PT_W, H = PT_H;
  const float pxmm = W / 210.0f; /* пікселів на міліметр */
  uint32_t seed = 9137u + (uint32_t)var * 7919u, st = seed;
  float *img = malloc((size_t)W * H * 3 * sizeof(float));
  /* плавні шари рахуємо на сітці 4×4 пікселі й інтерполюємо — так у 16 разів швидше */
  const int G = 4, GW = W / G + 2, GH = H / G + 2;
  float *gm = malloc((size_t)GW * GH * 3 * sizeof(float));
  for (int gy = 0; gy < GH; gy++)
    for (int gx = 0; gx < GW; gx++) {
      float u_ = gx * G / pxmm, v_ = gy * G / pxmm;
      float *q = &gm[((size_t)gy * GW + gx) * 3];
      q[0] = fbm(u_ / 38, v_ / 38, 5, seed) - 0.5f;   /* загальна строкатість */
      q[1] = fbm(u_ / 30 + 50, v_ / 30, 5, seed + 7);  /* кілька великих водяних плям */
      q[2] = fbm(u_ / 5, v_ / 5, 3, seed + 13) - 0.5f; /* нерівність краю */
    }
  /* основа + пожовтіння великими плямами + вигоряння з одного боку */
  float side = rnd(&st) < 0.5f ? 1 : -1, tilt = 0.4f + rnd(&st) * 0.6f;
  for (int y = 0; y < H; y++)
    for (int x = 0; x < W; x++) {
      float u_ = x / pxmm, v_ = y / pxmm;
      float fx = (float)x / G, fy = (float)y / G;
      int ix = (int)fx, iy = (int)fy;
      float ax = fx - ix, ay = fy - iy, L[3];
      const float *q00 = &gm[((size_t)iy * GW + ix) * 3], *q10 = q00 + 3, *q01 = q00 + GW * 3, *q11 = q01 + 3;
      for (int c = 0; c < 3; c++) L[c] = (q00[c] * (1 - ax) + q10[c] * ax) * (1 - ay) + (q01[c] * (1 - ax) + q11[c] * ax) * ay;
      float m = L[0], bl = L[1];
      float grad = ((x / (float)W) - 0.5f) * side * 0.5f + ((y / (float)H) - 0.5f) * tilt * 0.3f;
      float r = 236, g = 222, b = 186;
      float yel = m * 0.9f + grad * 0.35f;                        /* >0 — жовтіше й темніше */
      r -= yel * 14; g -= yel * 22; b -= yel * 40;
      /* водяні плями: м'яке потемніння всередині й тонкий темніший обідок по краю */
      float stain = smooth01(0.64f, 0.82f, bl) * 0.8f;
      float tide = (smooth01(0.630f, 0.648f, bl) - smooth01(0.648f, 0.70f, bl)) * 0.55f;
      r -= stain * 9 + tide * 10; g -= stain * 15 + tide * 17; b -= stain * 24 + tide * 26;
      /* краї: темніші, нерівні, сильніше в кутах */
      float dx = fminf(u_, 210 - u_), dy = fminf(v_, 297 - v_);
      float d = fminf(dx, dy) + L[2] * 7.0f;
      float corner = expf(-(dx * dx + dy * dy) / (2 * 22 * 22));
      float e = expf(-fmaxf(d, 0) / 6.5f) * 0.55f + corner * 0.25f;
      r -= e * 70; g -= e * 92; b -= e * 108;
      float *pp = &img[((size_t)y * W + x) * 3];
      pp[0] = r; pp[1] = g; pp[2] = b;
    }
  free(gm);
  /* іржаві цятки */
  int nspots = 28 + (int)(rnd(&st) * 30);
  for (int k = 0; k < nspots; k++) {
    float cx = rnd(&st) * W, cy = rnd(&st) * H;
    float rad = (0.25f + powf(rnd(&st), 2.2f) * 2.6f) * pxmm, inten = 0.35f + rnd(&st) * 0.65f;
    int x0 = (int)(cx - rad * 3), x1 = (int)(cx + rad * 3), y0 = (int)(cy - rad * 3), y1 = (int)(cy + rad * 3);
    for (int y = y0 < 0 ? 0 : y0; y <= y1 && y < H; y++)
      for (int x = x0 < 0 ? 0 : x0; x <= x1 && x < W; x++) {
        float ddx = (x - cx) / rad, ddy = (y - cy) / rad;
        float dd = ddx * ddx + ddy * ddy;
        float n = vnoise(x / (rad * 0.6f), y / (rad * 0.6f), seed + k);
        float a = expf(-dd * (1.2f + n)) * inten;
        float *pp = &img[((size_t)y * W + x) * 3];
        pp[0] -= a * 38; pp[1] -= a * 62; pp[2] -= a * 80;
      }
  }
  /* слід від чашки (не на всіх аркушах) */
  if (var != 1) {
    float cx = (0.25f + rnd(&st) * 0.5f) * W, cy = (0.55f + rnd(&st) * 0.35f) * H, R0 = (24 + rnd(&st) * 14) * pxmm;
    int x0 = (int)(cx - R0 * 1.3f), x1 = (int)(cx + R0 * 1.3f), y0 = (int)(cy - R0 * 1.3f), y1 = (int)(cy + R0 * 1.3f);
    for (int y = y0 < 0 ? 0 : y0; y <= y1 && y < H; y++)
      for (int x = x0 < 0 ? 0 : x0; x <= x1 && x < W; x++) {
        float ddx = x - cx, ddy = y - cy, dist = sqrtf(ddx * ddx + ddy * ddy);
        float ang = atan2f(ddy, ddx);
        float rr = R0 * (1 + (vnoise(ang * 3 + 10, 0.5f, seed + 31) - 0.5f) * 0.08f);
        float ring = expf(-powf((dist - rr) / (0.7f * pxmm), 2)) * (0.35f + vnoise(ang * 6, 1.5f, seed + 37) * 0.65f) *
                     smooth01(0.25f, 0.55f, vnoise(ang * 1.6f + 3, 2.5f, seed + 39)); /* обідок місцями переривається */
        float inside = dist < rr ? 0.05f * (0.5f + vnoise(x / 40.0f, y / 40.0f, seed + 41)) : 0;
        float a = ring * 0.32f + inside;
        float *pp = &img[((size_t)y * W + x) * 3];
        pp[0] -= a * 40; pp[1] -= a * 66; pp[2] -= a * 86;
      }
  }
  /* згин: світла смуга й темна лінія */
  {
    float fy = H * (0.5f + (rnd(&st) - 0.5f) * 0.04f);
    for (int y = (int)fy - (int)(3 * pxmm); y < fy + 3 * pxmm && y < H; y++)
      for (int x = 0; x < W; x++) {
        float off = (vnoise(x / 60.0f, 3.0f, seed + 51) - 0.5f) * 0.8f * pxmm;
        float dd = (y - fy - off) / pxmm;
        float dark = expf(-dd * dd / 0.12f) * 0.35f, light = dd < 0 ? expf(-dd * dd / 2.5f) * 0.3f : 0;
        float wear = 0.6f + vnoise(x / 25.0f, 7.0f, seed + 53) * 0.8f;
        float *pp = &img[((size_t)y * W + x) * 3];
        pp[0] += (light * 10 - dark * 34) * wear; pp[1] += (light * 10 - dark * 40) * wear; pp[2] += (light * 8 - dark * 44) * wear;
      }
  }
  /* волокна */
  int nfib = 5200;
  for (int k = 0; k < nfib; k++) {
    float cx = rnd(&st) * W, cy = rnd(&st) * H, ang = rnd(&st) * 6.2832f, len = (0.5f + rnd(&st) * 2.8f) * pxmm;
    float curv = (rnd(&st) - 0.5f) * 0.6f, dv = (rnd(&st) < 0.75f ? -1 : 1) * (1.5f + rnd(&st) * 3.0f);
    for (float t = 0; t < len; t += 0.6f) {
      float a2 = ang + curv * t / len;
      int x = (int)(cx + cosf(a2) * t), y = (int)(cy + sinf(a2) * t);
      if (x < 0 || y < 0 || x >= W || y >= H) break;
      float *pp = &img[((size_t)y * W + x) * 3];
      pp[0] += dv; pp[1] += dv; pp[2] += dv * 0.9f;
    }
  }
  /* дрібне зерно й запис */
  for (int y = 0; y < H; y++)
    for (int x = 0; x < W; x++) {
      float *pp = &img[((size_t)y * W + x) * 3];
      float gr = (h01(x, y, seed + 99) - 0.5f) * 5.0f + (vnoise(x / 3.0f, y / 3.0f, seed + 77) - 0.5f) * 4.0f;
      for (int c = 0; c < 3; c++) {
        float vv = pp[c] + gr;
        out[((size_t)y * W + x) * 3 + c] = (unsigned char)(vv < 0 ? 0 : vv > 255 ? 255 : vv);
      }
    }
  free(img);
}

static int paper_thread(void *arg) {
  int i = (int)(intptr_t)arg; /* кожен варіант — у своєму потоці */
  unsigned char *b = malloc((size_t)PT_W * PT_H * 3);
  gen_paper(i, b);
  unsigned char *sm = malloc((size_t)PZ_W * PZ_H * 3);
  for (int y = 0; y < PZ_H; y++)
    for (int x = 0; x < PZ_W; x++) {
      float fx = (x + 0.5f) * PT_W / PZ_W - 0.5f, fy = (y + 0.5f) * PT_H / PZ_H - 0.5f;
      int x0 = (int)fx, y0 = (int)fy, x1 = x0 + 1 < PT_W ? x0 + 1 : x0, y1 = y0 + 1 < PT_H ? y0 + 1 : y0;
      float ax = fx - x0, ay = fy - y0;
      for (int c = 0; c < 3; c++) {
        float v = (b[((size_t)y0 * PT_W + x0) * 3 + c] * (1 - ax) + b[((size_t)y0 * PT_W + x1) * 3 + c] * ax) * (1 - ay) +
                  (b[((size_t)y1 * PT_W + x0) * 3 + c] * (1 - ax) + b[((size_t)y1 * PT_W + x1) * 3 + c] * ax) * ay;
        sm[((size_t)y * PZ_W + x) * 3 + c] = (unsigned char)(v + 0.5f);
      }
    }
  uLongf zn = compressBound((uLong)PZ_W * PZ_H * 3);
  unsigned char *z = malloc(zn);
  compress2(z, &zn, sm, (uLong)PZ_W * PZ_H * 3, 6);
  free(sm);
  paper_z[i] = z;
  paper_zn[i] = zn;
  paper_rgb[i] = b;
  SDL_AtomicSet(&paper_var_ok[i], 1);
  SDL_AtomicAdd(&paper_ready, 1);
  return 0;
}

static void paper_start(void) {
  if (paper_started) return;
  paper_started = 1;
  for (int i = 0; i < NPT; i++) SDL_DetachThread(SDL_CreateThread(paper_thread, "paper", (void *)(intptr_t)i));
}

/* Текстура для сторінки p або NULL, якщо ще генерується. */
static SDL_Texture *paper_for(int p) {
  paper_start();
  int i = p % NPT;
  if (!SDL_AtomicGet(&paper_var_ok[i])) { /* поки цей варіант не готовий — будь-який готовий */
    i = -1;
    for (int k = 0; k < NPT; k++) if (SDL_AtomicGet(&paper_var_ok[k])) { i = k; break; }
    if (i < 0) return NULL;
  }
  if (!paper_tex[i]) {
    SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "1");
    paper_tex[i] = SDL_CreateTexture(R, SDL_PIXELFORMAT_RGB24, SDL_TEXTUREACCESS_STATIC, PT_W, PT_H);
    if (paper_tex[i]) SDL_UpdateTexture(paper_tex[i], NULL, paper_rgb[i], PT_W * 3);
    SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "0");
  }
  return paper_tex[i];
}

/* ================= PDF ================= */
typedef struct { char *p; size_t n, cap; } Buf;
static void bput(Buf *b, const void *d, size_t n) {
  if (b->n + n > b->cap) { b->cap = (b->n + n) * 2 + 4096; b->p = realloc(b->p, b->cap); }
  memcpy(b->p + b->n, d, n);
  b->n += n;
}
static void bprintf(Buf *b, const char *fmt, ...) {
  char tmp[4096];
  va_list ap;
  va_start(ap, fmt);
  int k = vsnprintf(tmp, sizeof tmp, fmt, ap);
  va_end(ap);
  bput(b, tmp, k);
}

static unsigned char *deflate_buf(const void *d, size_t n, size_t *outn) {
  uLongf cap = compressBound(n);
  unsigned char *o = malloc(cap);
  compress2(o, &cap, d, n, 9);
  *outn = cap;
  return o;
}

/* Один рядок тексту в PDF: кожен гліф на своїй позиції сітки машинки. */
static void pdf_row(Buf *cs, const uint32_t *cp, int n, int row, int col0, uint32_t *uni, int NG) {
  hb_glyph_info_t *inf;
  unsigned g = shape(&fdoc, cp, n, &inf, NULL);
  double y = PAGE_H - (MARGIN_TOP + row * LEAD_PT + (LEAD_PT - FONT_PT * 7.0 / 6.0) / 2 + BASEPT);
  unsigned last = (unsigned)-1;
  for (unsigned i = 0; i < g; i++) {
    unsigned cl = inf[i].cluster;
    if (i == 0 || cl != last + 1) {
      if (i) bprintf(cs, "> Tj\n");
      bprintf(cs, "1 0 0 1 %.3f %.3f Tm <", MARGIN_X + (col0 + cl) * ADVPT, y);
    }
    last = cl;
    unsigned gid = inf[i].codepoint;
    if ((int)gid < NG && !uni[gid]) uni[gid] = cp[cl];
    bprintf(cs, "%04X", gid & 0xFFFF);
  }
  if (g) bprintf(cs, "> Tj\n");
}

static int export_pdf(const char *outpath) {
  if (!have_doc_font || !D) return 0;
  int np = npages(), NG = fdoc.ng;
  uint32_t *uni = calloc(NG, sizeof(uint32_t));
  Buf out = {0};
  RGB ink = INKS[S.ink], paper = PAPERS[S.paper];
  int print_bw = S.pdfmode == 1; /* для принтера: білий аркуш, чисто чорний шрифт */
  if (print_bw) ink = (RGB){0, 0, 0};
  /* старий папір у PDF — та сама текстура, що й на екрані (картинка під текстом) */
  int nimg = 0;
  if (S.paper == 2 && !print_bw) {
    paper_start();
    for (int t = 0; t < 600 && SDL_AtomicGet(&paper_ready) < NPT; t++) SDL_Delay(10);
    if (SDL_AtomicGet(&paper_ready) >= NPT) nimg = np < NPT ? np : NPT;
  }
  int img0 = 8 + 2 * np; /* номери об'єктів картинок */
  long *off = calloc((size_t)(8 + 2 * np + nimg), sizeof(long));
  int nobj = 7 + 2 * np + nimg;
#define OBJ(i) (off[i] = (long)out.n, bprintf(&out, "%d 0 obj\n", i))
  bprintf(&out, "%%PDF-1.4\n%%\xE2\xE3\xCF\xD3\n");
  OBJ(1); bprintf(&out, "<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
  OBJ(2); bprintf(&out, "<< /Type /Pages /Count %d /Kids [", np);
  for (int p = 0; p < np; p++) bprintf(&out, "%d 0 R ", 8 + 2 * p);
  bprintf(&out, "] >>\nendobj\n");

  for (int p = 0; p < np; p++) {
    Buf cs = {0};
    if (nimg) bprintf(&cs, "q %.3f 0 0 %.3f 0 0 cm /Im%d Do Q\n", PAGE_W, PAGE_H, p % nimg);
    else if (S.paper != 0 && !print_bw)
      bprintf(&cs, "q %.3f %.3f %.3f rg 0 0 %.3f %.3f re f Q\n", paper.r / 255.0, paper.g / 255.0, paper.b / 255.0, PAGE_W, PAGE_H);
    bprintf(&cs, "BT\n/F1 %.1f Tf\n%.3f %.3f %.3f rg\n", FONT_PT, ink.r / 255.0, ink.g / 255.0, ink.b / 255.0);
    for (int r = 0; r < TPP; r++) {
      int l = p * TPP + r;
      if (l >= D->nL) break;
      if (!D->L[l].len) continue;
      pdf_row(&cs, D->c + D->L[l].start, D->L[l].len, r + HDR, 0, uni, NG);
    }
    for (int which = 0; which < 2; which++) {
      int mode = which == 0 ? S.header : S.pagenum;
      if (mode != 2 || (which == 0 ? !HDR : !FTR)) continue;
      char dt[256];
      deco_text(p, which, dt, sizeof dt);
      int dn;
      uint32_t *dc = utf8_dec(dt, strlen(dt), &dn);
      int col = which == 0 ? 0 : COLS - dn;
      pdf_row(&cs, dc, dn, which == 0 ? 0 : LPP - 1, col < 0 ? 0 : col, uni, NG);
      free(dc);
    }
    bprintf(&cs, "ET\n");
    size_t zn;
    unsigned char *z = deflate_buf(cs.p, cs.n, &zn);
    OBJ(8 + 2 * p);
    char xo[64] = "";
    if (nimg) snprintf(xo, sizeof xo, " /XObject << /Im%d %d 0 R >>", p % nimg, img0 + p % nimg);
    bprintf(&out, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 %.3f %.3f] /Resources << /Font << /F1 3 0 R >>%s >> /Contents %d 0 R >>\nendobj\n",
            PAGE_W, PAGE_H, xo, 9 + 2 * p);
    OBJ(9 + 2 * p);
    bprintf(&out, "<< /Length %zu /Filter /FlateDecode >>\nstream\n", zn);
    bput(&out, z, zn);
    bprintf(&out, "\nendstream\nendobj\n");
    free(z);
    free(cs.p);
  }

  double k = 1000.0 / fdoc.upem;
  FT_Face fc = fdoc.face;
  OBJ(3);
  bprintf(&out, "<< /Type /Font /Subtype /Type0 /BaseFont /%s /Encoding /Identity-H /DescendantFonts [4 0 R] /ToUnicode 7 0 R >>\nendobj\n", doc_psname);
  OBJ(4);
  bprintf(&out, "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /%s /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> "
                "/FontDescriptor 5 0 R /DW %d /CIDToGIDMap /Identity >>\nendobj\n", doc_psname, (int)lround(doc_adv * k));
  OBJ(5);
  bprintf(&out, "<< /Type /FontDescriptor /FontName /%s /Flags 5 /FontBBox [%d %d %d %d] /ItalicAngle 0 /Ascent %d /Descent %d "
                "/CapHeight %d /StemV 80 /FontFile2 6 0 R >>\nendobj\n", doc_psname,
          (int)lround(fc->bbox.xMin * k), (int)lround(fc->bbox.yMin * k), (int)lround(fc->bbox.xMax * k), (int)lround(fc->bbox.yMax * k),
          (int)lround(fc->ascender * k), (int)lround(fc->descender * k), (int)lround(fc->ascender * k));
  size_t fzn;
  unsigned char *fz = deflate_buf(fdoc.data, fdoc.len, &fzn);
  OBJ(6);
  bprintf(&out, "<< /Length %zu /Length1 %ld /Filter /FlateDecode >>\nstream\n", fzn, fdoc.len);
  bput(&out, fz, fzn);
  bprintf(&out, "\nendstream\nendobj\n");
  free(fz);

  Buf cm = {0};
  bprintf(&cm, "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n"
               "/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
  int cnt = 0, done = 0;
  for (int g = 0; g < NG && g < 65536; g++) if (uni[g]) cnt++;
  for (int g = 0; g < NG && g < 65536; g++) {
    if (!uni[g]) continue;
    if (done % 100 == 0) bprintf(&cm, "%d beginbfchar\n", cnt - done < 100 ? cnt - done : 100);
    uint32_t v = uni[g];
    if (v > 0xFFFF) { v -= 0x10000; bprintf(&cm, "<%04X> <%04X%04X>\n", g, 0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF)); }
    else bprintf(&cm, "<%04X> <%04X>\n", g, v);
    done++;
    if (done % 100 == 0 || done == cnt) bprintf(&cm, "endbfchar\n");
  }
  bprintf(&cm, "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
  OBJ(7);
  bprintf(&out, "<< /Length %zu >>\nstream\n", cm.n);
  bput(&out, cm.p, cm.n);
  bprintf(&out, "\nendstream\nendobj\n");
  free(cm.p);

  for (int i = 0; i < nimg; i++) {
    OBJ(img0 + i);
    bprintf(&out, "<< /Type /XObject /Subtype /Image /Width %d /Height %d /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length %lu >>\nstream\n", PZ_W, PZ_H, paper_zn[i]);
    bput(&out, paper_z[i], paper_zn[i]);
    bprintf(&out, "\nendstream\nendobj\n");
  }
  long xref = (long)out.n;
  bprintf(&out, "xref\n0 %d\n0000000000 65535 f \n", nobj + 1);
  for (int i = 1; i <= nobj; i++) bprintf(&out, "%010ld 00000 n \n", off[i]);
  bprintf(&out, "trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%ld\n%%%%EOF\n", nobj + 1, xref);
#undef OBJ
  FILE *f = fopen(outpath, "wb");
  int ok = f && fwrite(out.p, 1, out.n, f) == out.n;
  if (f) fclose(f);
  free(out.p); free(off); free(uni);
  return ok;
}


/* ================= машинний код ================= */
#define BIN_HEX_MAX (4u << 20)   /* hex показуємо для перших 4 МБ */
#define BIN_ASM_MAX (48u << 20)  /* стільки тексту асемблера беремо щонайбільше */


/* Запускає програму без оболонки й забирає її вивід (не більше max байтів). */
static char *run_capture2(char *const argv[], size_t max, size_t *outn, int with_err) {
  int pfd[2];
  if (pipe(pfd)) return NULL;
  pid_t pid = fork();
  if (pid < 0) { close(pfd[0]); close(pfd[1]); return NULL; }
  if (pid == 0) {
    dup2(pfd[1], 1);
    if (with_err) dup2(pfd[1], 2);
    else { int dn = open("/dev/null", O_WRONLY); if (dn >= 0) dup2(dn, 2); }
    close(pfd[0]); close(pfd[1]);
    execvp(argv[0], argv);
    _exit(127);
  }
  close(pfd[1]);
  size_t n = 0, cap = 1 << 16;
  char *b = malloc(cap);
  int cut = 0;
  for (;;) {
    if (n + 65536 > cap) { cap *= 2; b = realloc(b, cap); }
    ssize_t k = read(pfd[0], b + n, 65536);
    if (k < 0 && errno == EINTR) continue;
    if (k <= 0) break;
    n += (size_t)k;
    if (n >= max) { cut = 1; break; }
  }
  close(pfd[0]);
  if (cut) kill(pid, SIGTERM);
  int st = 0;
  waitpid(pid, &st, 0);
  if (!n) { free(b); return NULL; } /* є вивід — беремо, навіть якщо програма поскаржилась */
  *outn = n;
  return b;
}
static char *run_capture(char *const argv[], size_t max, size_t *outn) { return run_capture2(argv, max, outn, 0); }

/* Табуляції у виводі objdump розгортаємо до позицій, кратних 8, щоб стовпці не роз'їжджалися. */
static char *expand_tabs(const char *s, size_t n, size_t *outn) {
  size_t cap = n + n / 2 + 64, k = 0;
  char *o = malloc(cap);
  int col = 0;
  for (size_t i = 0; i < n; i++) {
    if (k + 16 > cap) { cap *= 2; o = realloc(o, cap); }
    char c = s[i];
    if (c == '\t') { do { o[k++] = ' '; col++; } while (col % 8); }
    else { o[k++] = c; if (c == '\n') col = 0; else if (((unsigned char)c & 0xC0) != 0x80) col++; }
  }
  *outn = k;
  return o;
}

/* Класичний hex-дамп: зсув, 16 байтів, ті самі байти як текст. */
static char *hex_dump(const char *p, size_t *outn) {
  FILE *f = fopen(p, "rb");
  if (!f) return NULL;
  fseek(f, 0, SEEK_END);
  long total = ftell(f);
  fseek(f, 0, SEEK_SET);
  size_t n = total > (long)BIN_HEX_MAX ? BIN_HEX_MAX : (size_t)total;
  unsigned char *d = malloc(n + 1);
  n = fread(d, 1, n, f);
  fclose(f);
  static const char HX[] = "0123456789abcdef";
  size_t lines = (n + 15) / 16, k = 0;
  char *o = malloc(lines * 80 + 256);
  if ((size_t)total > n)
    k += (size_t)sprintf(o, "; showing the first %u MB of %.1f MB\n\n", BIN_HEX_MAX >> 20, total / 1048576.0);
  for (size_t off = 0; off < n; off += 16) {
    k += (size_t)sprintf(o + k, "%08zx  ", off);
    for (int i = 0; i < 16; i++) {
      if (off + i < n) { o[k++] = HX[d[off + i] >> 4]; o[k++] = HX[d[off + i] & 15]; o[k++] = ' '; }
      else { o[k++] = ' '; o[k++] = ' '; o[k++] = ' '; }
      if (i == 7) o[k++] = ' ';
    }
    o[k++] = ' ';
    for (int i = 0; i < 16 && off + i < n; i++) { unsigned char c = d[off + i]; o[k++] = c >= 32 && c < 127 ? (char)c : '.'; }
    o[k++] = '\n';
  }
  free(d);
  *outn = k;
  return o;
}

/* ================= типи файлів =================
 * Тип визначається за вмістом (сигнатурою), а не за розширенням — як у nano чи file(1).
 * Текст: UTF-8, UTF-16 (з BOM або без), Windows-1251; закінчення рядків CRLF зберігаються. */

static const uint16_t CP1251[128] = {
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0xFFFD, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F};

static int has_at(const unsigned char *b, size_t n, size_t off, const char *sig, size_t k) { return n >= off + k && !memcmp(b + off, sig, k); }

static int valid_utf8(const unsigned char *b, size_t n, int allow_cut) {
  for (size_t i = 0; i < n;) {
    unsigned c = b[i];
    int k = c < 0x80 ? 0 : (c & 0xE0) == 0xC0 ? 1 : (c & 0xF0) == 0xE0 ? 2 : (c & 0xF8) == 0xF0 ? 3 : -1;
    if (k < 0 || (k == 1 && c < 0xC2)) return 0;
    if (i + k >= n) return allow_cut; /* обрізано посеред символу на межі зразка */
    for (int j = 1; j <= k; j++) if ((b[i + j] & 0xC0) != 0x80) return 0;
    i += (size_t)k + 1;
  }
  return 1;
}

/* Визначає тип файлу; для тексту ще й кодування. */
static int file_type(const char *p, int *enc) {
  unsigned char b[65536];
  FILE *f = fopen(p, "rb");
  if (enc) *enc = ENC_UTF8;
  if (!f) return FT_TEXT;
  size_t n = fread(b, 1, sizeof b, f);
  long total;
  fseek(f, 0, SEEK_END);
  total = ftell(f);
  fclose(f);
  if (n == 0) return FT_TEXT;
  if (has_at(b, n, 0, "\xEF\xBB\xBF", 3)) return FT_TEXT;
  if (has_at(b, n, 0, "\xFF\xFE", 2) && !has_at(b, n, 0, "\xFF\xFE\0\0", 4)) { if (enc) *enc = ENC_UTF16LE; return FT_TEXT; }
  if (has_at(b, n, 0, "\xFE\xFF", 2)) { if (enc) *enc = ENC_UTF16BE; return FT_TEXT; }
  if (has_at(b, n, 0, "%PDF", 4)) return FT_PDF;
  if (has_at(b, n, 0, "\x89PNG", 4) || has_at(b, n, 0, "\xFF\xD8\xFF", 3) || has_at(b, n, 0, "GIF8", 4) ||
      (has_at(b, n, 0, "RIFF", 4) && has_at(b, n, 8, "WEBP", 4)) || has_at(b, n, 0, "II*\0", 4) || has_at(b, n, 0, "MM\0*", 4) ||
      has_at(b, n, 0, "\xFF\x0A", 2) || has_at(b, n, 4, "JXL ", 4) || has_at(b, n, 0, "qoif", 4) ||
      (has_at(b, n, 4, "ftyp", 4) && (has_at(b, n, 8, "avif", 4) || has_at(b, n, 8, "heic", 4) || has_at(b, n, 8, "heix", 4) || has_at(b, n, 8, "mif1", 4))) ||
      (has_at(b, n, 0, "BM", 2) && n > 26 && (long)(b[2] | b[3] << 8 | b[4] << 16 | (long)b[5] << 24) == total))
    return FT_IMAGE;
  if (has_at(b, n, 0, "\x7f" "ELF", 4)) return FT_ELF;
  if (has_at(b, n, 0, "PK\x03\x04", 4)) {
    if (memmem(b, n, "word/", 5)) return FT_DOCX;
    if (memmem(b, n, "application/epub", 16)) return FT_EPUB;
    if (memmem(b, n, "application/vnd.oasis.opendocument.text", 39)) return FT_ODT;
    return FT_ARCHIVE;
  }
  if (has_at(b, n, 0, "\x1F\x8B", 2) || has_at(b, n, 0, "\xFD" "7zXZ", 5) || has_at(b, n, 0, "\x28\xB5\x2F\xFD", 4) ||
      has_at(b, n, 0, "BZh", 3) || has_at(b, n, 0, "7z\xBC\xAF\x27\x1C", 6) || has_at(b, n, 0, "Rar!", 4) || has_at(b, n, 257, "ustar", 5) ||
      has_at(b, n, 0, "\xED\xAB\xEE\xDB", 4) || has_at(b, n, 0, "!<arch>", 7))
    return FT_ARCHIVE;
  if (has_at(b, n, 0, "ID3", 3) || has_at(b, n, 0, "OggS", 4) || has_at(b, n, 0, "fLaC", 4) || has_at(b, n, 0, "\x1A\x45\xDF\xA3", 4) ||
      (has_at(b, n, 0, "RIFF", 4) && (has_at(b, n, 8, "WAVE", 4) || has_at(b, n, 8, "AVI ", 4))) || has_at(b, n, 4, "ftyp", 4) ||
      (n > 2 && b[0] == 0xFF && (b[1] & 0xE0) == 0xE0 && b[1] != 0xFF))
    return FT_MEDIA;
  /* без підпису: текст чи ні? */
  size_t zeros = 0, zero_odd = 0, zero_even = 0, ctrl = 0;
  for (size_t i = 0; i < n; i++) {
    if (!b[i]) { zeros++; if (i & 1) zero_odd++; else zero_even++; }
    else if (b[i] < 32 && b[i] != '\n' && b[i] != '\r' && b[i] != '\t' && b[i] != '\f' && b[i] != 27 && b[i] != '\b') ctrl++;
  }
  if (zeros) { /* UTF-16 без BOM: нулі через байт */
    if (zero_odd > n * 0.3 && zero_even < n * 0.02) { if (enc) *enc = ENC_UTF16LE; return FT_TEXT; }
    if (zero_even > n * 0.3 && zero_odd < n * 0.02) { if (enc) *enc = ENC_UTF16BE; return FT_TEXT; }
    return FT_BINARY;
  }
  if (valid_utf8(b, n, n == sizeof b)) return ctrl * 100 > n ? FT_BINARY : FT_TEXT;
  if (ctrl * 100 <= n) { if (enc) *enc = ENC_CP1251; return FT_TEXT; } /* стара кирилиця */
  return FT_BINARY;
}

/* Сирі байти будь-якого кодування → UTF-8 (без BOM). */
static char *to_utf8(const unsigned char *b, size_t n, int enc, size_t *outn) {
  char *o = malloc(n * 3 + 8);
  size_t k = 0;
  if (enc == ENC_UTF8) {
    if (n >= 3 && !memcmp(b, "\xEF\xBB\xBF", 3)) { b += 3; n -= 3; }
    memcpy(o, b, n);
    k = n;
  } else if (enc == ENC_CP1251) {
    for (size_t i = 0; i < n; i++) k += (size_t)utf8_enc(b[i] < 0x80 ? b[i] : CP1251[b[i] - 0x80], o + k);
  } else {
    int be = enc == ENC_UTF16BE;
    size_t i = 0;
    if (n >= 2 && ((!be && b[0] == 0xFF && b[1] == 0xFE) || (be && b[0] == 0xFE && b[1] == 0xFF))) i = 2;
    o = realloc(o, n * 2 + 8);
    for (; i + 1 < n; i += 2) {
      uint32_t c = be ? (uint32_t)(b[i] << 8 | b[i + 1]) : (uint32_t)(b[i + 1] << 8 | b[i]);
      if (c >= 0xD800 && c < 0xDC00 && i + 3 < n) {
        uint32_t c2 = be ? (uint32_t)(b[i + 2] << 8 | b[i + 3]) : (uint32_t)(b[i + 3] << 8 | b[i + 2]);
        if (c2 >= 0xDC00 && c2 < 0xE000) { c = 0x10000 + ((c - 0xD800) << 10) + (c2 - 0xDC00); i += 2; }
      }
      k += (size_t)utf8_enc(c, o + k);
    }
  }
  *outn = k;
  return o;
}

/* Текст документа → байти у його кодуванні, з його закінченнями рядків. Повертає кількість незакодованих символів. */
static int from_doc(Doc *d, char **out, size_t *outn) {
  size_t cap = (size_t)d->n * 4 + 16, k = 0;
  unsigned char *o = malloc(cap * 2);
  int bad = 0;
  if (d->enc == ENC_UTF8 && d->bom) { memcpy(o, "\xEF\xBB\xBF", 3); k = 3; }
  if (d->enc == ENC_UTF16LE) { o[k++] = 0xFF; o[k++] = 0xFE; }
  if (d->enc == ENC_UTF16BE) { o[k++] = 0xFE; o[k++] = 0xFF; }
  for (int i = 0; i < d->n; i++) {
    uint32_t c = d->c[i];
    for (int rep = 0; rep < (c == '\n' && d->crlf ? 2 : 1); rep++) {
      uint32_t ch = (c == '\n' && d->crlf && rep == 0) ? '\r' : c;
      if (d->enc == ENC_UTF8) k += (size_t)utf8_enc(ch, (char *)o + k);
      else if (d->enc == ENC_CP1251) {
        int byte = -1;
        if (ch < 0x80) byte = (int)ch;
        else for (int j = 0; j < 128; j++) if (CP1251[j] == ch) { byte = 0x80 + j; break; }
        if (byte < 0) { byte = '?'; bad++; }
        o[k++] = (unsigned char)byte;
      } else {
        int be = d->enc == ENC_UTF16BE;
        uint32_t units[2];
        int nu = 1;
        if (ch >= 0x10000) { units[0] = 0xD800 + ((ch - 0x10000) >> 10); units[1] = 0xDC00 + ((ch - 0x10000) & 0x3FF); nu = 2; }
        else units[0] = ch;
        for (int q = 0; q < nu; q++) {
          o[k++] = (unsigned char)(be ? units[q] >> 8 : units[q] & 0xFF);
          o[k++] = (unsigned char)(be ? units[q] & 0xFF : units[q] >> 8);
        }
      }
    }
  }
  *out = (char *)o;
  *outn = k;
  return bad;
}

/* XML документа (docx/odt/epub) → простий текст: абзаци й рядки зберігаються, теги зникають. */
static char *xml_to_text(const char *x, size_t n, size_t *outn) {
  char *o = malloc(n + 16);
  size_t k = 0;
  int nl = 0;
  for (size_t i = 0; i < n;) {
    if (x[i] == '<') {
      size_t e = i + 1;
      while (e < n && x[e] != '>') e++;
      size_t tl = e - i - 1;
      const char *t = x + i + 1;
#define TAGIS(s) (tl >= strlen(s) && !strncmp(t, s, strlen(s)))
      if (TAGIS("/w:p") || TAGIS("/text:p") || TAGIS("/text:h") || TAGIS("/p") || TAGIS("/h1") || TAGIS("/h2") || TAGIS("/h3") ||
          TAGIS("/li") || TAGIS("br") || TAGIS("w:br") || TAGIS("text:line-break") || TAGIS("/div") || TAGIS("/tr")) {
        if (nl < 2) { o[k++] = '\n'; nl++; }
      } else if (TAGIS("w:tab") || TAGIS("text:tab") || TAGIS("/w:tc") || TAGIS("/td") || TAGIS("/th")) {
        o[k++] = ' '; o[k++] = ' ';
      }
#undef TAGIS
      i = e + 1;
      continue;
    }
    if (x[i] == '&') {
      static const char *ent[][2] = {{"&amp;", "&"}, {"&lt;", "<"}, {"&gt;", ">"}, {"&quot;", "\""}, {"&apos;", "'"}, {"&#160;", " "}, {"&nbsp;", " "}};
      int hitk = 0;
      for (int q = 0; q < 7; q++) {
        size_t el = strlen(ent[q][0]);
        if (i + el <= n && !strncmp(x + i, ent[q][0], el)) { o[k++] = ent[q][1][0]; i += el; hitk = 1; break; }
      }
      if (hitk) { nl = 0; continue; }
    }
    if (x[i] == '\n' || x[i] == '\r') { i++; continue; } /* розмітка, не текст */
    o[k++] = x[i++];
    nl = 0;
  }
  *outn = k;
  return o;
}

static char *run_capture2(char *const argv[], size_t max, size_t *outn, int with_err);

/* Текстове подання нетекстового файлу для вигляду VIEW_EXTRACT. */
static char *extract_text(const char *path, int ft, size_t *n) {
  char p[2048];
  snprintf(p, sizeof p, "%s", path);
  size_t rn = 0;
  char *raw = NULL;
  if (ft == FT_PDF) {
    char *a[] = {"pdftotext", "-layout", "-enc", "UTF-8", p, "-", NULL};
    raw = run_capture2(a, 64u << 20, &rn, 0);
  } else if (ft == FT_DOCX || ft == FT_ODT || ft == FT_EPUB) {
    char *a1[] = {"bsdtar", "-xOf", p, "word/document.xml", NULL}, *a2[] = {"bsdtar", "-xOf", p, "content.xml", NULL};
    char *a3[] = {"bsdtar", "-xOf", p, "*.xhtml", "*.html", "*.htm", NULL};
    char *xml = run_capture2(ft == FT_DOCX ? a1 : ft == FT_ODT ? a2 : a3, 128u << 20, &rn, 0);
    if (!xml && ft != FT_EPUB) { char *u[] = {"unzip", "-p", p, ft == FT_DOCX ? "word/document.xml" : "content.xml", NULL}; xml = run_capture2(u, 128u << 20, &rn, 0); }
    if (xml) { raw = xml_to_text(xml, rn, &rn); free(xml); }
  } else if (ft == FT_ARCHIVE) {
    char *a[] = {"bsdtar", "-tvf", p, NULL};
    raw = run_capture2(a, 32u << 20, &rn, 0);
    if (!raw) { char *u[] = {"unzip", "-l", p, NULL}; raw = run_capture2(u, 32u << 20, &rn, 0); }
  } else if (ft == FT_MEDIA || ft == FT_IMAGE) {
    char *a[] = {"ffprobe", "-hide_banner", p, NULL};
    raw = run_capture2(a, 1u << 20, &rn, 1);
    if (!raw) { char *b[] = {"file", "-b", p, NULL}; raw = run_capture2(b, 1u << 20, &rn, 0); }
  }
  *n = rn;
  return raw;
}

/* ================= зображення =================
 * Декодування через SDL2_image (підвантажується під час роботи, збирати з ним не треба).
 * Якщо його немає — через magick / convert / ffmpeg у формат PPM. */
#include <dlfcn.h>
static SDL_Surface *(*p_IMG_Load)(const char *);
static int img_tried;
static void img_lib(void) {
  if (img_tried) return;
  img_tried = 1;
  const char *names[] = {"libSDL2_image-2.0.so.0", "libSDL2_image.so", "libSDL2_image-2.0.so"};
  for (int i = 0; i < 3 && !p_IMG_Load; i++) {
    void *h = dlopen(names[i], RTLD_NOW | RTLD_LOCAL);
    if (h) *(void **)&p_IMG_Load = dlsym(h, "IMG_Load");
  }
}

static SDL_Surface *ppm_surface(const unsigned char *b, size_t n) {
  if (n < 16 || b[0] != 'P' || b[1] != '6') return NULL;
  size_t i = 2;
  long v[3];
  for (int q = 0; q < 3; q++) {
    while (i < n && (b[i] == ' ' || b[i] == '\n' || b[i] == '\r' || b[i] == '\t' || b[i] == '#')) {
      if (b[i] == '#') while (i < n && b[i] != '\n') i++;
      else i++;
    }
    v[q] = 0;
    while (i < n && b[i] >= '0' && b[i] <= '9') v[q] = v[q] * 10 + (b[i++] - '0');
  }
  i++;
  long w = v[0], h = v[1], mx = v[2];
  int bpc = mx > 255 ? 2 : 1;
  if (w <= 0 || h <= 0 || (size_t)(w * h * 3 * bpc) > n - i) return NULL;
  SDL_Surface *s = SDL_CreateRGBSurfaceWithFormat(0, (int)w, (int)h, 32, SDL_PIXELFORMAT_ARGB8888);
  if (!s) return NULL;
  for (long y = 0; y < h; y++) {
    uint32_t *row = (uint32_t *)((char *)s->pixels + y * s->pitch);
    for (long x = 0; x < w; x++) {
      const unsigned char *px = b + i + (size_t)(y * w + x) * 3 * bpc;
      unsigned r = px[0], g = px[bpc], bl = px[2 * bpc];
      row[x] = 0xFF000000u | r << 16 | g << 8 | bl;
    }
  }
  return s;
}

/* Зменшення вдвічі усередненням — без драбинок навіть для величезних фото. */
static SDL_Surface *half(SDL_Surface *s) {
  int w = s->w / 2, h = s->h / 2;
  SDL_Surface *o = SDL_CreateRGBSurfaceWithFormat(0, w, h, 32, SDL_PIXELFORMAT_ARGB8888);
  for (int y = 0; y < h; y++) {
    uint32_t *a = (uint32_t *)((char *)s->pixels + 2 * y * s->pitch), *b = (uint32_t *)((char *)s->pixels + (2 * y + 1) * s->pitch);
    uint32_t *d = (uint32_t *)((char *)o->pixels + y * o->pitch);
    for (int x = 0; x < w; x++) {
      uint32_t p[4] = {a[2 * x], a[2 * x + 1], b[2 * x], b[2 * x + 1]}, r = 0;
      for (int sh = 0; sh < 32; sh += 8) {
        unsigned sum = 0;
        for (int k = 0; k < 4; k++) sum += (p[k] >> sh) & 0xFF;
        r |= (uint32_t)((sum + 2) / 4) << sh;
      }
      d[x] = r;
    }
  }
  SDL_FreeSurface(s);
  return o;
}

static SDL_Surface *load_image(const char *path) {
  SDL_Surface *s = p_IMG_Load ? p_IMG_Load(path) : NULL;
  if (!s) s = SDL_LoadBMP(path);
  if (!s) {
    char p[2100], p0[2100];
    snprintf(p, sizeof p, "%s", path);
    snprintf(p0, sizeof p0, "%s[0]", path);
    char *a1[] = {"magick", p0, "ppm:-", NULL}, *a2[] = {"convert", p0, "ppm:-", NULL};
    char *a3[] = {"ffmpeg", "-v", "error", "-i", p, "-frames:v", "1", "-f", "image2pipe", "-vcodec", "ppm", "-", NULL};
    char **tries[] = {a1, a2, a3};
    for (int t = 0; t < 3 && !s; t++) {
      size_t n;
      char *b = run_capture2(tries[t], 512u << 20, &n, 0);
      if (b) { s = ppm_surface((unsigned char *)b, n); free(b); }
    }
  }
  if (!s) return NULL;
  if (s->format->format != SDL_PIXELFORMAT_ARGB8888) {
    SDL_Surface *c = SDL_ConvertSurfaceFormat(s, SDL_PIXELFORMAT_ARGB8888, 0);
    SDL_FreeSurface(s);
    s = c;
    if (!s) return NULL;
  }
  while (s->w > 4096 || s->h > 4096) s = half(s);
  return s;
}

/* ================= перегляд: зображення, слайдшоу, PDF ================= */
typedef struct Media_ {
  SDL_Texture *tex; int tw, th; char cur[2048], shown[2048]; /* що завантажується і що показане */
  char **files; int nf;                       /* слайдшоу: скільки зображень знайдено при останньому скануванні */
  char **hist; int nh, hpos; unsigned serial; /* історія показаних шляхів; номер останнього запиту */
  Uint32 next_t; int paused, waiting;
  int pages; float pw, ph;                    /* PDF: кількість сторінок і розмір у пунктах */
  SDL_Texture **pt; int *pstate;              /* 0 немає, 1 рендериться, 2 готова, 3 помилка */
  Uint32 *pused;
  char err[300];
} Media;

/* Фонові завдання: декодування картинки або рендер сторінки PDF. */
/* page >= 0 — сторінка PDF; page == -1 — картинка path; page == -2 — слайдшоу: у теці path
   вибрати випадкове зображення (не avoid) і декодувати; результат кладеться в path. */
typedef struct Job { struct Job *next; unsigned uid, serial; int page, count, same, push; char path[2048], avoid[2048]; SDL_Surface *surf; SDL_atomic_t done; } Job;
static void scan_images(struct Media_ *m, const char *dir, int depth);
static void media_files_free(struct Media_ *m);
static Job *jobs;
static SDL_mutex *jobs_mx;

static int job_thread(void *arg) {
  Job *j = arg;
  if (j->page >= 0) { /* сторінка PDF: pdftoppm пише PPM прямо у вивід */
    char pg[16], p[2048];
    snprintf(pg, sizeof pg, "%d", j->page + 1);
    snprintf(p, sizeof p, "%s", j->path);
    char *a[] = {"pdftoppm", "-f", pg, "-l", pg, "-r", "150", p, NULL};
    size_t n;
    char *b = run_capture2(a, 256u << 20, &n, 0);
    if (b) { j->surf = ppm_surface((unsigned char *)b, n); free(b); }
  } else if (j->page == -2) {
    /* Теку скануємо щоразу заново: файли могли зникнути або з'явитися. */
    struct Media_ *tmp = calloc(1, sizeof(struct Media_));
    scan_images(tmp, j->path, 0);
    j->count = tmp->nf;
    unsigned seed = (unsigned)SDL_GetPerformanceCounter() ^ (j->serial * 2654435761u);
    for (int tries = 0; tmp->nf > 0 && tries < 32; tries++) {
      int k = (int)(rand_r(&seed) % (unsigned)tmp->nf);
      if (tmp->nf > 1 && !strcmp(tmp->files[k], j->avoid)) { tries--; continue; }
      if (!strcmp(tmp->files[k], j->avoid)) { j->same = 1; snprintf(j->path, sizeof j->path, "%s", tmp->files[k]); break; }
      SDL_Surface *s = load_image(tmp->files[k]);
      if (s) { j->surf = s; snprintf(j->path, sizeof j->path, "%s", tmp->files[k]); break; }
      /* не відкрилося (зник, ще пишеться, битий) — прибираємо зі списку й пробуємо інший */
      free(tmp->files[k]);
      tmp->files[k] = tmp->files[--tmp->nf];
    }
    media_files_free(tmp);
    free(tmp);
  } else j->surf = load_image(j->path);
  SDL_AtomicSet(&j->done, 1);
  return 0;
}

static void job_start2(Doc *d, const char *path, int page, const char *avoid, unsigned serial, int push) {
  if (!jobs_mx) jobs_mx = SDL_CreateMutex();
  Job *j = calloc(1, sizeof(Job));
  j->uid = d->uid;
  j->page = page;
  j->serial = serial;
  j->push = push;
  snprintf(j->path, sizeof j->path, "%s", path);
  if (avoid) snprintf(j->avoid, sizeof j->avoid, "%s", avoid);
  SDL_LockMutex(jobs_mx);
  j->next = jobs; jobs = j;
  SDL_UnlockMutex(jobs_mx);
  SDL_DetachThread(SDL_CreateThread(job_thread, "job", j));
}

static void job_start(Doc *d, const char *path, int page) { job_start2(d, path, page, NULL, 0, 0); }

static int is_image_name(const char *n) {
  static const char *ext[] = {".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp", ".tif", ".tiff", ".avif", ".jxl", ".heic", ".heif", ".tga", ".qoi", ".jfif", ".pnm", ".ppm"};
  size_t l = strlen(n);
  for (size_t i = 0; i < sizeof ext / sizeof *ext; i++) {
    size_t e = strlen(ext[i]);
    if (l > e && !strcasecmp(n + l - e, ext[i])) return 1;
  }
  return 0;
}

static void scan_images(Media *m, const char *dir, int depth) {
  if (depth > 6 || m->nf >= 50000) return;
  DIR *dd = opendir(dir);
  if (!dd) return;
  struct dirent *de;
  while ((de = readdir(dd))) {
    if (de->d_name[0] == '.') continue;
    char full[2400];
    snprintf(full, sizeof full, "%s/%s", dir, de->d_name);
    if (is_dir(full)) scan_images(m, full, depth + 1);
    else if (is_image_name(de->d_name)) {
      if (!(m->nf & 255)) m->files = realloc(m->files, (size_t)(m->nf + 256) * sizeof(char *));
      m->files[m->nf++] = strdup(full);
    }
  }
  closedir(dd);
}

static void media_files_free(Media *m) {
  for (int i = 0; i < m->nf; i++) free(m->files[i]);
  free(m->files);
  m->files = NULL;
  m->nf = 0;
}

static void media_free(Media *m) {
  if (!m) return;
  if (m->tex) SDL_DestroyTexture(m->tex);
  media_files_free(m);
  for (int i = 0; i < m->nh; i++) free(m->hist[i]);
  free(m->hist);
  for (int i = 0; i < m->pages; i++) if (m->pt && m->pt[i]) SDL_DestroyTexture(m->pt[i]);
  free(m->pt); free(m->pstate); free(m->pused);
  free(m);
}

/* Слайдшоу: нічого не вибирається заздалегідь. Кожен крок «далі» сканує теку заново у фоні
   й бере випадковий файл, який є саме зараз; зниклі файли просто не потрапляють у вибір. */
static void slide_next(Doc *d) {
  Media *m = d->media;
  if (m->waiting) return;
  if (m->hpos + 1 < m->nh && file_exists(m->hist[m->hpos + 1])) { /* після «назад» — знову вперед по історії */
    m->hpos++;
    snprintf(m->cur, sizeof m->cur, "%s", m->hist[m->hpos]);
    m->waiting = 1;
    job_start(d, m->cur, -1);
    return;
  }
  m->serial++;
  m->cur[0] = 0;
  m->waiting = 1;
  job_start2(d, d->path, -2, m->shown, m->serial, 1);
}

static void slide_prev(Doc *d) {
  Media *m = d->media;
  if (m->waiting) return;
  for (int i = m->hpos - 1; i >= 0; i--) /* пропускаємо в історії те, чого вже немає на диску */
    if (file_exists(m->hist[i])) {
      m->hpos = i;
      snprintf(m->cur, sizeof m->cur, "%s", m->hist[i]);
      m->waiting = 1;
      job_start(d, m->cur, -1);
      return;
    }
}

static void slide_push(Media *m, const char *p) {
  if (m->nh && m->hpos >= 0 && !strcmp(m->hist[m->hpos], p)) return;
  for (int i = m->hpos + 1; i < m->nh; i++) free(m->hist[i]); /* нова гілка після «назад» */
  m->nh = m->hpos + 1;
  if (m->nh >= 1000) { free(m->hist[0]); memmove(m->hist, m->hist + 1, (size_t)(m->nh - 1) * sizeof(char *)); m->nh--; }
  m->hist = realloc(m->hist, (size_t)(m->nh + 1) * sizeof(char *));
  m->hist[m->nh] = strdup(p);
  m->hpos = m->nh++;
}

/* Відкриває вигляд з картинкою / слайдшоу / PDF у документі d. 0 — не вдалося. */
static int media_open(Doc *d, const char *p, int view) {
  img_lib();
  Media *m = calloc(1, sizeof(Media));
  if (view == VIEW_SLIDES) {
    m->hpos = -1; /* файли не збираємо заздалегідь: кожен показ сканує теку заново */
  } else if (view == VIEW_PDF) {
    char pp[2048];
    snprintf(pp, sizeof pp, "%s", p);
    char *a[] = {"pdfinfo", pp, NULL};
    size_t n;
    char *info = run_capture2(a, 1u << 20, &n, 0);
    if (!info) { free(m); return 0; } /* немає poppler — покажемо текст або hex */
    info = realloc(info, n + 1);
    info[n] = 0;
    char *pg = strstr(info, "Pages:"), *sz = strstr(info, "Page size:");
    m->pages = pg ? atoi(pg + 6) : 0;
    m->pw = 595.276f; m->ph = 841.89f;
    if (sz) sscanf(sz + 10, " %f x %f", &m->pw, &m->ph);
    free(info);
    if (m->pages <= 0) { free(m); return 0; }
    m->pt = calloc((size_t)m->pages, sizeof(SDL_Texture *));
    m->pstate = calloc((size_t)m->pages, sizeof(int));
    m->pused = calloc((size_t)m->pages, sizeof(Uint32));
  }
  media_free(d->media);
  d->media = m;
  d->n = 0; d->cur = d->anchor = 0;
  snprintf(d->path, sizeof d->path, "%s", p);
  d->ro = 1; d->view = view; d->dirty = 0; d->scroll = 0; d->ver++;
  clear_history(d);
  layout(d);
  if (view == VIEW_IMAGE) { snprintf(m->cur, sizeof m->cur, "%s", p); m->waiting = 1; job_start(d, p, -1); }
  if (view == VIEW_SLIDES) slide_next(d);
  return 1;
}

/* Забирає готові фонові завдання (головний потік: тільки тут можна створювати текстури). */
static void jobs_collect(void) {
  if (!jobs_mx) return;
  SDL_LockMutex(jobs_mx);
  Job **pj = &jobs;
  while (*pj) {
    Job *j = *pj;
    if (!SDL_AtomicGet(&j->done)) { pj = &j->next; continue; }
    *pj = j->next;
    Doc *d = NULL;
    for (int i = 0; i < ndocs; i++) if (docs[i]->uid == j->uid) d = docs[i];
    Media *m = d ? d->media : NULL;
    if (m && j->page >= 0 && j->page < m->pages && strcmp(d->path, j->path) == 0) {
      if (j->surf) { SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "1"); m->pt[j->page] = SDL_CreateTextureFromSurface(R, j->surf); SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "0"); }
      m->pstate[j->page] = j->surf ? 2 : 3;
    } else if (m && j->page == -2 && d->view == VIEW_SLIDES && j->serial == m->serial && m->waiting && !m->cur[0]) {
      m->nf = j->count;
      if (j->surf) {
        if (m->tex) SDL_DestroyTexture(m->tex);
        SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "1");
        m->tex = SDL_CreateTextureFromSurface(R, j->surf);
        SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "0");
        m->tw = j->surf->w; m->th = j->surf->h;
        m->err[0] = 0;
        snprintf(m->shown, sizeof m->shown, "%s", j->path);
        slide_push(m, j->path);
      } else if (j->same) {
        m->err[0] = 0; /* у теці лише поточне зображення — лишаємо його */
      } else {
        /* нічого не показали: старе зображення прибираємо, лише якщо його файлу вже немає */
        if (m->shown[0] && !file_exists(m->shown)) {
          if (m->tex) SDL_DestroyTexture(m->tex);
          m->tex = NULL;
          m->shown[0] = 0;
        }
        if (!m->tex) snprintf(m->err, sizeof m->err, "%s", j->count ? "Could not open the pictures in this folder" : "There are no pictures in this folder now");
      }
      m->waiting = 0;
      m->next_t = SDL_GetTicks() + 5000;
    } else if (m && j->page == -1 && !strcmp(m->cur, j->path) && d->view == VIEW_SLIDES && !j->surf) {
      /* файл з історії зник саме зараз — лишаємо поточну картинку й одразу беремо випадкову */
      m->waiting = 0;
      m->next_t = SDL_GetTicks();
    } else if (m && j->page == -1 && !strcmp(m->cur, j->path)) {
      if (m->tex) SDL_DestroyTexture(m->tex);
      m->tex = NULL;
      if (j->surf) {
        SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "1");
        m->tex = SDL_CreateTextureFromSurface(R, j->surf);
        SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, "0");
        m->tw = j->surf->w; m->th = j->surf->h;
        m->err[0] = 0;
        snprintf(m->shown, sizeof m->shown, "%s", j->path);
      } else snprintf(m->err, sizeof m->err, "Could not open: %s%s", basename_of(j->path),
                      p_IMG_Load ? "" : " (install sdl2_image)");
      m->waiting = 0;
      m->next_t = SDL_GetTicks() + 5000;
    }
    if (j->surf) SDL_FreeSurface(j->surf);
    free(j);
  }
  SDL_UnlockMutex(jobs_mx);
}


/* Відкриває двійковий файл як текст лише для читання. Повертає вигляд, який вдалося показати. */
static int load_view(Doc *d, const char *p, int view) {
  size_t n = 0;
  char *t = NULL;
  if (!d->ftype) d->ftype = file_type(p, NULL);
  if (is_media_view(view)) {
    if (media_open(d, p, view)) return view;
    view = d->ftype == FT_PDF ? VIEW_EXTRACT : VIEW_HEX; /* немає програм для показу — текст або hex */
  }
  media_free(d->media);
  d->media = NULL;
  if (view == VIEW_EXTRACT) {
    size_t rn;
    char *raw = extract_text(p, d->ftype, &rn);
    if (raw) {
      size_t un;
      char *u8 = to_utf8((unsigned char *)raw, rn, valid_utf8((unsigned char *)raw, rn, 0) ? ENC_UTF8 : ENC_CP1251, &un);
      free(raw);
      t = expand_tabs(u8, un, &n);
      free(u8);
    } else view = VIEW_HEX;
  }
  if (view == VIEW_ASM) {
    char path[2048];
    snprintf(path, sizeof path, "%s", p);
    char *argv[] = {"objdump", "-d", "-w", "-C", "-M", "intel", path, NULL};
    size_t rn;
    char *raw = run_capture(argv, BIN_ASM_MAX, &rn);
    if (raw) { t = expand_tabs(raw, rn, &n); free(raw); }
    else view = VIEW_HEX; /* objdump не впорався — показуємо hex */
  }
  if (view == VIEW_HEX) t = hex_dump(p, &n);
  if (!t) { t = strdup("; could not read the file\n"); n = strlen(t); }
  int m;
  uint32_t *cp = utf8_dec(t, n, &m);
  free(t);
  reserve(d, m);
  memcpy(d->c, cp, (size_t)m * sizeof(uint32_t));
  d->n = m;
  free(cp);
  if (p != d->path) snprintf(d->path, sizeof d->path, "%s", p);
  d->ro = 1;
  d->view = view;
  d->enc = ENC_UTF8; d->crlf = 0; d->bom = 0;
  d->cur = d->anchor = 0;
  d->dirty = 0;
  d->scroll = 0;
  d->ver++;
  clear_history(d);
  layout(d);
  return view;
}

/* ================= збереження, PDF, закриття, вихід =================
 * Жодних діалогів: назва файлу вводиться в полі праворуч, PDF зберігається одразу
 * поруч із документом, закриття зі змінами підтверджується повторним натисканням. */
static int modal; /* завжди 0: спливаючих вікон немає */
static int font_missing;

static void join_path(char *out, size_t n, const char *dir, const char *name) {
  snprintf(out, n, "%s/%s", strcmp(dir, "/") ? dir : "", name);
}

static void name_focus(void);
static void act_save(void) {
  if (!D) return;
  if (!D->path[0] || D->ro) { name_focus(); set_toast("Type a file name in the field on the right and press Enter"); return; }
  save_to(D, D->path);
}

/* Зберегти як: назва з поля праворуч; відносна — від теки документа або дерева. */
static void save_as_name(const char *name) {
  if (!D || !*name) return;
  char full[2400], dir[1024];
  if (name[0] == '/') snprintf(full, sizeof full, "%s", name);
  else if (name[0] == '~' && name[1] == '/') { char h[1024]; home_dir(h, sizeof h, NULL); snprintf(full, sizeof full, "%s%s", h, name + 1); }
  else {
    if (D->path[0]) dirname_of(D->path, dir, sizeof dir); else snprintf(dir, sizeof dir, "%s", root);
    join_path(full, sizeof full, dir, name);
  }
  if (is_dir(full)) { set_toast("This is a folder: add a file name"); return; }
  if (save_to(D, full)) tree_rebuild();
}

static int export_pdf(const char *outpath);
/* PDF поруч із документом: notes.txt → notes.pdf, main.c → main.c.pdf */
static void pdf_path_for(Doc *d, char *out, size_t n) {
  if (!d->path[0]) { join_path(out, n, root, "Untitled.pdf"); return; }
  char p[2048];
  snprintf(p, sizeof p, "%s", d->path);
  char *sl = strrchr(p, '/'), *dot = strrchr(p, '.');
  if (dot && (!sl || dot > sl + 1) && (!strcasecmp(dot, ".txt") || !strcasecmp(dot, ".md") || !strcasecmp(dot, ".text"))) *dot = 0;
  snprintf(out, n, "%s.pdf", p);
}

static void act_pdf(void) {
  if (!D) return;
  char p[2400], m[2600];
  pdf_path_for(D, p, sizeof p);
  if (export_pdf(p)) { snprintf(m, sizeof m, "PDF saved: %s", basename_of(p)); tree_rebuild(); }
  else snprintf(m, sizeof m, "Could not write %s", basename_of(p));
  set_toast(m);
}

static Doc *close_arm_doc;
static Uint32 close_arm_t;
static void ask_close(int i) {
  if (i < 0 || i >= ndocs) return;
  Doc *d = docs[i];
  if (d->dirty && !(close_arm_doc == d && SDL_GetTicks() - close_arm_t < 4000)) {
    activate(i);
    close_arm_doc = d;
    close_arm_t = SDL_GetTicks();
    set_toast("Unsaved changes. Close without saving: Ctrl+W or × again; save: Ctrl+S");
    return;
  }
  close_arm_doc = NULL;
  close_tab_now(i);
}

/* Вихід нічого не питає: незбережені зміни потрапляють у сеанс і повернуться під час запуску. */
static void session_save(void);
static void quit_step(void) {
  session_save();
  running = 0;
}

/* ================= дерево файлів ================= */
typedef struct { char path[1024]; char name[256]; int depth, dir; } Node;
static Node *nodes;
static int nnodes, capnodes, tree_sel = -1;
static char **expanded;
static int nexp, capexp;
static double tree_scroll;
static Uint32 tree_t;

static int find_expanded(const char *p) {
  for (int i = 0; i < nexp; i++) if (!strcmp(expanded[i], p)) return i;
  return -1;
}

static void toggle_expanded(const char *p) {
  int i = find_expanded(p);
  if (i >= 0) { free(expanded[i]); expanded[i] = expanded[--nexp]; return; }
  if (nexp == capexp) { capexp = capexp ? capexp * 2 : 32; expanded = realloc(expanded, capexp * sizeof(char *)); }
  expanded[nexp++] = strdup(p);
}

static int node_cmp(const void *a, const void *b) {
  const Node *x = a, *y = b;
  if (x->dir != y->dir) return y->dir - x->dir;
  return strcasecmp(x->name, y->name);
}

static void tree_add_dir(const char *dir, int depth) {
  if (depth > 12) return;
  DIR *d = opendir(dir);
  if (!d) return;
  Node *tmp = NULL;
  int nt = 0, ct = 0;
  struct dirent *de;
  while ((de = readdir(d))) {
    if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) continue;
    if (de->d_name[0] == '.' && !S.hidden) continue;
    if (nt == ct) { ct = ct ? ct * 2 : 32; tmp = realloc(tmp, ct * sizeof(Node)); }
    Node *n = &tmp[nt++];
    join_path(n->path, sizeof n->path, dir, de->d_name);
    snprintf(n->name, sizeof n->name, "%s", de->d_name);
    n->depth = depth;
    n->dir = is_dir(n->path);
  }
  closedir(d);
  if (nt) qsort(tmp, nt, sizeof(Node), node_cmp);
  for (int i = 0; i < nt; i++) {
    if (nnodes == capnodes) { capnodes = capnodes ? capnodes * 2 : 128; nodes = realloc(nodes, capnodes * sizeof(Node)); }
    nodes[nnodes++] = tmp[i];
    if (tmp[i].dir && find_expanded(tmp[i].path) >= 0) tree_add_dir(tmp[i].path, depth + 1);
  }
  free(tmp);
}

static void tree_rebuild(void) {
  char keep[1024] = "";
  if (tree_sel >= 0 && tree_sel < nnodes) snprintf(keep, sizeof keep, "%s", nodes[tree_sel].path);
  nnodes = 0;
  tree_add_dir(root, 0);
  tree_sel = -1;
  for (int i = 0; i < nnodes && keep[0]; i++) if (!strcmp(nodes[i].path, keep)) tree_sel = i;
  tree_t = SDL_GetTicks();
}

enum { F_EDIT, F_FILES, F_TERM };
static int focus = F_EDIT;

/* Тека вище: дерево показує батьківську теку, попередня лишається розгорнутою. */
static void tree_up(void) {
  if (!strcmp(root, "/")) return;
  char old[1024], up[1024];
  snprintf(old, sizeof old, "%s", root);
  dirname_of(root, up, sizeof up);
  snprintf(root, sizeof root, "%s", up);
  if (find_expanded(old) < 0) toggle_expanded(old);
  tree_rebuild();
  for (int i = 0; i < nnodes; i++) if (!strcmp(nodes[i].path, old)) tree_sel = i;
  tree_scroll = 0;
}

/* Зайти в теку: вона стає коренем дерева (клік по назві теки або Enter). */
static void tree_enter(int i) {
  if (i < 0 || i >= nnodes || !nodes[i].dir) return;
  snprintf(root, sizeof root, "%s", nodes[i].path);
  tree_sel = -1;
  tree_scroll = 0;
  tree_rebuild();
}

static void tree_activate(int i) {
  if (i < 0 || i >= nnodes) return;
  tree_sel = i;
  if (nodes[i].dir) { toggle_expanded(nodes[i].path); tree_rebuild(); return; }
  open_file(nodes[i].path);
}
/* ================= кошик =================
 * Delete у провіднику переносить файл чи теку в кошик робочого столу
 * (~/.local/share/Trash, як у файлових менеджерах). Ctrl+Z там же — повернути. */
static void mkdirs(const char *path);
static struct { char orig[2048], files[2400], info[2400]; int ok; } last_trash;

static int run_wait(char *const argv[]) {
  pid_t pid = fork();
  if (pid < 0) return -1;
  if (pid == 0) {
    int dn = open("/dev/null", O_WRONLY);
    if (dn >= 0) { dup2(dn, 1); dup2(dn, 2); }
    execvp(argv[0], argv);
    _exit(127);
  }
  int st = 0;
  waitpid(pid, &st, 0);
  return WIFEXITED(st) ? WEXITSTATUS(st) : -1;
}

static void url_encode(const char *s, char *o, size_t n) {
  static const char HX[] = "0123456789ABCDEF";
  size_t k = 0;
  for (; *s && k + 4 < n; s++) {
    unsigned char c = (unsigned char)*s;
    if ((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || strchr("/-_.~", c)) o[k++] = (char)c;
    else { o[k++] = '%'; o[k++] = HX[c >> 4]; o[k++] = HX[c & 15]; }
  }
  o[k] = 0;
}

/* 1 — у кошику (можна повернути), 2 — через gio (інший диск), 0 — не вдалося */
static int trash_file(const char *path) {
  char base[1100], files[1200], info[1200];
  const char *x = getenv("XDG_DATA_HOME");
  if (x && *x) snprintf(base, sizeof base, "%s/Trash", x);
  else { char h[1024]; home_dir(h, sizeof h, NULL); snprintf(base, sizeof base, "%s/.local/share/Trash", h); }
  snprintf(files, sizeof files, "%s/files", base);
  snprintf(info, sizeof info, "%s/info", base);
  mkdirs(files);
  mkdirs(info);
  const char *nm = basename_of(path);
  char dst[2400], inf[2400], cand[600];
  struct stat st;
  for (int k = 1;; k++) {
    if (k == 1) snprintf(cand, sizeof cand, "%s", nm); else snprintf(cand, sizeof cand, "%s.%d", nm, k);
    snprintf(dst, sizeof dst, "%s/%s", files, cand);
    snprintf(inf, sizeof inf, "%s/%s.trashinfo", info, cand);
    if (lstat(dst, &st) != 0 && lstat(inf, &st) != 0) break;
    if (k > 9999) return 0;
  }
  int fdi = open(inf, O_WRONLY | O_CREAT | O_EXCL, 0600);
  if (fdi < 0) return 0;
  char enc[6200], date[64], buf[6400];
  url_encode(path, enc, sizeof enc);
  time_t t = time(NULL);
  struct tm tm;
  localtime_r(&t, &tm);
  strftime(date, sizeof date, "%Y-%m-%dT%H:%M:%S", &tm);
  int bn = snprintf(buf, sizeof buf, "[Trash Info]\nPath=%s\nDeletionDate=%s\n", enc, date);
  int okw = write(fdi, buf, (size_t)bn) == bn;
  close(fdi);
  if (okw && rename(path, dst) == 0) {
    snprintf(last_trash.orig, sizeof last_trash.orig, "%s", path);
    snprintf(last_trash.files, sizeof last_trash.files, "%s", dst);
    snprintf(last_trash.info, sizeof last_trash.info, "%s", inf);
    last_trash.ok = 1;
    return 1;
  }
  int err = errno;
  unlink(inf);
  if (err == EXDEV) { /* файл на іншому диску — хай кошик того диска знайде gio */
    char pp[2048];
    snprintf(pp, sizeof pp, "%s", path);
    char *argv[] = {"gio", "trash", pp, NULL};
    if (run_wait(argv) == 0) return 2;
  }
  return 0;
}

static void tree_trash(void) {
  if (tree_sel < 0 || tree_sel >= nnodes) return;
  char p[1024], nm[300];
  snprintf(p, sizeof p, "%s", nodes[tree_sel].path);
  snprintf(nm, sizeof nm, "%s", nodes[tree_sel].name);
  size_t pl = strlen(p);
  int res = trash_file(p);
  if (!res) { char m[400]; snprintf(m, sizeof m, "Could not move “%s” to the bin", nm); set_toast(m); return; }
  /* закриваємо вкладки з цього файлу чи теки, якщо в них немає незбережених змін */
  for (int i = ndocs - 1; i >= 0; i--) {
    const char *dp = docs[i]->path;
    if ((!strcmp(dp, p) || (!strncmp(dp, p, pl) && dp[pl] == '/')) && !docs[i]->dirty) close_tab_now(i);
  }
  int keep = tree_sel;
  tree_rebuild();
  tree_sel = keep < nnodes ? keep : nnodes - 1;
  char m[400];
  snprintf(m, sizeof m, res == 1 ? "“%s” is in the bin · Ctrl+Z brings it back" : "“%s” is in the bin", nm);
  set_toast(m);
}

static void trash_undo(void) {
  struct stat st;
  if (!last_trash.ok) { set_toast("Nothing to bring back"); return; }
  if (lstat(last_trash.orig, &st) == 0) { set_toast("A file with that name is already in its old place"); return; }
  if (rename(last_trash.files, last_trash.orig) != 0) { set_toast("Could not bring it back from the bin"); return; }
  unlink(last_trash.info);
  last_trash.ok = 0;
  tree_rebuild();
  for (int i = 0; i < nnodes; i++) if (!strcmp(nodes[i].path, last_trash.orig)) tree_sel = i;
  set_toast("Brought back from the bin");
}

static const RGB C_TERM_BG = {26, 25, 23}, C_TERM_FG = {218, 213, 203};
/* ================= термінал ================= */
typedef struct { uint32_t ch; RGB fg, bg; Uint8 fl; } Cell;
enum { CF_BOLD = 1, CF_INV = 2, CF_DFG = 4, CF_DBG = 8, CF_UL = 16 };
typedef struct { Cell *c; int n; } HLine;
#define HMAX 3000

static const RGB ANSI16[16] = {
    {46, 44, 41}, {214, 96, 84}, {138, 180, 104}, {222, 180, 92}, {108, 152, 214}, {190, 128, 196}, {96, 184, 178}, {210, 204, 194},
    {110, 106, 100}, {240, 122, 108}, {164, 206, 128}, {240, 204, 120}, {136, 178, 240}, {214, 156, 220}, {124, 210, 204}, {248, 244, 236}};

static Cell *scr, *scr_alt, pen;
static int trows, tcols, tcx, tcy, t_wrap, t_top, t_bot, t_alt, t_curvis = 1, t_appcur, t_scx, t_scy;
static HLine *hist;
static int nhist, hview;
static int pty_fd = -1, term_dead;
static pid_t child_pid;
static Uint32 EV_PTY;
static float t_cw, t_ch;

enum { PS_GND, PS_ESC, PS_CSI, PS_OSC, PS_OSC_ESC, PS_SKIP1 };
static int ps;
static char csi[128];
static int csil;
static uint32_t ucp;
static int uneed;

static RGB color256(int n) {
  if (n < 16) return ANSI16[n];
  if (n < 232) {
    n -= 16;
    int r = n / 36, g = (n / 6) % 6, b = n % 6;
    RGB c = {(Uint8)(r ? 55 + r * 40 : 0), (Uint8)(g ? 55 + g * 40 : 0), (Uint8)(b ? 55 + b * 40 : 0)};
    return c;
  }
  Uint8 v = (Uint8)(8 + (n - 232) * 10);
  return (RGB){v, v, v};
}

static Cell blank_cell(void) {
  Cell c = {' ', C_TERM_FG, pen.bg, (Uint8)(CF_DFG | (pen.fl & CF_DBG))};
  return c;
}

static void pen_reset(void) { pen.ch = ' '; pen.fg = C_TERM_FG; pen.bg = C_TERM_BG; pen.fl = CF_DFG | CF_DBG; }

#define CELL(r, c) scr[(r) * tcols + (c)]

static void clear_cells(Cell *row, int a, int b) {
  Cell bl = blank_cell();
  for (int i = a; i < b; i++) row[i] = bl;
}

static void hist_push(Cell *row, int n) {
  if (nhist == HMAX) { free(hist[0].c); memmove(hist, hist + 1, (HMAX - 1) * sizeof(HLine)); nhist--; }
  if (!hist) hist = calloc(HMAX, sizeof(HLine));
  hist[nhist].c = malloc(n * sizeof(Cell));
  memcpy(hist[nhist].c, row, n * sizeof(Cell));
  hist[nhist].n = n;
  nhist++;
}

static void scroll_up(int n) {
  for (int k = 0; k < n; k++) {
    if (t_top == 0 && !t_alt) hist_push(&CELL(0, 0), tcols);
    memmove(&CELL(t_top, 0), &CELL(t_top + 1, 0), (size_t)(t_bot - t_top) * tcols * sizeof(Cell));
    clear_cells(&CELL(t_bot, 0), 0, tcols);
  }
}

static void scroll_down(int n) {
  for (int k = 0; k < n; k++) {
    memmove(&CELL(t_top + 1, 0), &CELL(t_top, 0), (size_t)(t_bot - t_top) * tcols * sizeof(Cell));
    clear_cells(&CELL(t_top, 0), 0, tcols);
  }
}

static void lf(void) {
  if (tcy == t_bot) scroll_up(1);
  else if (tcy < trows - 1) tcy++;
}

static void put_char(uint32_t cp) {
  if (t_wrap) { tcx = 0; lf(); t_wrap = 0; }
  Cell c = pen;
  c.ch = cp;
  CELL(tcy, tcx) = c;
  if (tcx == tcols - 1) t_wrap = 1;
  else tcx++;
}

static Cell *resized(Cell *old, int orows, int ocols, int nr, int nc, int shift) {
  Cell *ns = malloc((size_t)nr * nc * sizeof(Cell));
  Cell bl = {' ', C_TERM_FG, C_TERM_BG, CF_DFG | CF_DBG};
  for (int i = 0; i < nr * nc; i++) ns[i] = bl;
  if (old)
    for (int r = 0; r < nr; r++) {
      int sr = r + shift;
      if (sr >= orows) break;
      for (int c = 0; c < nc && c < ocols; c++) ns[r * nc + c] = old[sr * ocols + c];
    }
  free(old);
  return ns;
}

static void term_resize(int nr, int nc) {
  if (nr < 2) nr = 2;
  if (nc < 10) nc = 10;
  if (nr == trows && nc == tcols && scr) return;
  int shift = scr ? tcy - (nr - 1) : 0;
  if (shift < 0) shift = 0;
  if (scr && !t_alt) for (int i = 0; i < shift; i++) hist_push(&scr[i * tcols], tcols);
  scr = resized(scr, trows, tcols, nr, nc, shift);
  scr_alt = resized(scr_alt, trows, tcols, nr, nc, 0);
  tcy -= shift;
  trows = nr; tcols = nc;
  if (tcx >= tcols) tcx = tcols - 1;
  if (tcy >= trows) tcy = trows - 1;
  if (tcy < 0) tcy = 0;
  t_top = 0; t_bot = trows - 1; t_wrap = 0;
  if (pty_fd >= 0) {
    struct winsize ws = {(unsigned short)trows, (unsigned short)tcols, 0, 0};
    ioctl(pty_fd, TIOCSWINSZ, &ws);
  }
}

static void pty_write(const char *s, size_t n) {
  if (pty_fd < 0) return;
  while (n > 0) {
    ssize_t w = write(pty_fd, s, n);
    if (w < 0) { if (errno == EINTR || errno == EAGAIN) continue; return; }
    s += w; n -= (size_t)w;
  }
}

static void set_alt(int on) {
  if (on == t_alt) return;
  Cell *t = scr; scr = scr_alt; scr_alt = t;
  t_alt = on;
  if (on) {
    t_scx = tcx; t_scy = tcy;
    for (int i = 0; i < trows * tcols; i++) scr[i] = (Cell){' ', C_TERM_FG, C_TERM_BG, CF_DFG | CF_DBG};
  } else { tcx = t_scx; tcy = t_scy; }
  t_wrap = 0;
}

static void sgr(int *P, int np) {
  if (np == 0) { pen_reset(); return; }
  for (int i = 0; i < np; i++) {
    int p = P[i];
    if (p == 0) pen_reset();
    else if (p == 1) pen.fl |= CF_BOLD;
    else if (p == 22) pen.fl &= ~CF_BOLD;
    else if (p == 4) pen.fl |= CF_UL;
    else if (p == 24) pen.fl &= ~CF_UL;
    else if (p == 7) pen.fl |= CF_INV;
    else if (p == 27) pen.fl &= ~CF_INV;
    else if (p >= 30 && p <= 37) { pen.fg = ANSI16[p - 30 + ((pen.fl & CF_BOLD) ? 8 : 0)]; pen.fl &= ~CF_DFG; }
    else if (p >= 90 && p <= 97) { pen.fg = ANSI16[p - 90 + 8]; pen.fl &= ~CF_DFG; }
    else if (p == 39) { pen.fg = C_TERM_FG; pen.fl |= CF_DFG; }
    else if (p >= 40 && p <= 47) { pen.bg = ANSI16[p - 40]; pen.fl &= ~CF_DBG; }
    else if (p >= 100 && p <= 107) { pen.bg = ANSI16[p - 100 + 8]; pen.fl &= ~CF_DBG; }
    else if (p == 49) { pen.bg = C_TERM_BG; pen.fl |= CF_DBG; }
    else if ((p == 38 || p == 48) && i + 1 < np) {
      RGB c;
      if (P[i + 1] == 5 && i + 2 < np) { c = color256(P[i + 2] & 255); i += 2; }
      else if (P[i + 1] == 2 && i + 4 < np) { c = (RGB){(Uint8)P[i + 2], (Uint8)P[i + 3], (Uint8)P[i + 4]}; i += 4; }
      else continue;
      if (p == 38) { pen.fg = c; pen.fl &= ~CF_DFG; } else { pen.bg = c; pen.fl &= ~CF_DBG; }
    }
  }
}

static void csi_dispatch(char fin) {
  int priv = csil > 0 && (csi[0] == '?' || csi[0] == '>' || csi[0] == '=');
  int P[16] = {0}, np = 0;
  const char *s = csi + (priv ? 1 : 0);
  if (*s) {
    np = 1;
    for (; *s && np <= 16; s++) {
      if (*s >= '0' && *s <= '9') P[np - 1] = P[np - 1] * 10 + (*s - '0');
      else if (*s == ';' || *s == ':') { if (np < 16) np++; else break; }
    }
  }
#define ARG(i, d) ((i) < np && P[i] > 0 ? P[i] : (d))
  if (!priv && strchr("ABCDEFGHfd`aerus", fin)) t_wrap = 0;
  if (priv && csi[0] == '?') {
    if (fin == 'h' || fin == 'l') {
      int on = fin == 'h';
      for (int i = 0; i < np; i++) {
        if (P[i] == 1) t_appcur = on;
        else if (P[i] == 25) t_curvis = on;
        else if (P[i] == 1049 || P[i] == 47 || P[i] == 1047) set_alt(on);
      }
    }
    return;
  }
  if (priv) {
    if (fin == 'c') pty_write("\x1b[>0;0;0c", 9);
    return;
  }
  switch (fin) {
    case 'A': tcy -= ARG(0, 1); if (tcy < 0) tcy = 0; break;
    case 'B': case 'e': tcy += ARG(0, 1); if (tcy >= trows) tcy = trows - 1; break;
    case 'C': case 'a': tcx += ARG(0, 1); if (tcx >= tcols) tcx = tcols - 1; break;
    case 'D': tcx -= ARG(0, 1); if (tcx < 0) tcx = 0; break;
    case 'E': tcy += ARG(0, 1); if (tcy >= trows) tcy = trows - 1; tcx = 0; break;
    case 'F': tcy -= ARG(0, 1); if (tcy < 0) tcy = 0; tcx = 0; break;
    case 'G': case '`': tcx = ARG(0, 1) - 1; if (tcx >= tcols) tcx = tcols - 1; break;
    case 'd': tcy = ARG(0, 1) - 1; if (tcy >= trows) tcy = trows - 1; break;
    case 'H': case 'f':
      tcy = ARG(0, 1) - 1; tcx = ARG(1, 1) - 1;
      if (tcy >= trows) tcy = trows - 1;
      if (tcx >= tcols) tcx = tcols - 1;
      break;
    case 'J': {
      int m = np ? P[0] : 0;
      if (m == 0) { clear_cells(&CELL(tcy, 0), tcx, tcols); for (int r = tcy + 1; r < trows; r++) clear_cells(&CELL(r, 0), 0, tcols); }
      else if (m == 1) { clear_cells(&CELL(tcy, 0), 0, tcx + 1); for (int r = 0; r < tcy; r++) clear_cells(&CELL(r, 0), 0, tcols); }
      else { for (int r = 0; r < trows; r++) clear_cells(&CELL(r, 0), 0, tcols); }
      break;
    }
    case 'K': {
      int m = np ? P[0] : 0;
      if (m == 0) clear_cells(&CELL(tcy, 0), tcx, tcols);
      else if (m == 1) clear_cells(&CELL(tcy, 0), 0, tcx + 1);
      else clear_cells(&CELL(tcy, 0), 0, tcols);
      break;
    }
    case 'L': if (tcy >= t_top && tcy <= t_bot) { int ot = t_top; t_top = tcy; scroll_down(ARG(0, 1)); t_top = ot; } break;
    case 'M': if (tcy >= t_top && tcy <= t_bot) { int ot = t_top; t_top = tcy; int sa = t_alt; t_alt = 1; scroll_up(ARG(0, 1)); t_alt = sa; t_top = ot; } break;
    case 'P': {
      int n = ARG(0, 1);
      if (n > tcols - tcx) n = tcols - tcx;
      memmove(&CELL(tcy, tcx), &CELL(tcy, tcx + n), (size_t)(tcols - tcx - n) * sizeof(Cell));
      clear_cells(&CELL(tcy, 0), tcols - n, tcols);
      break;
    }
    case '@': {
      int n = ARG(0, 1);
      if (n > tcols - tcx) n = tcols - tcx;
      memmove(&CELL(tcy, tcx + n), &CELL(tcy, tcx), (size_t)(tcols - tcx - n) * sizeof(Cell));
      clear_cells(&CELL(tcy, 0), tcx, tcx + n);
      break;
    }
    case 'X': { int n = ARG(0, 1); int e = tcx + n > tcols ? tcols : tcx + n; clear_cells(&CELL(tcy, 0), tcx, e); break; }
    case 'S': { int sa = t_alt; if (t_top != 0) t_alt = 1; scroll_up(ARG(0, 1)); t_alt = sa; break; }
    case 'T': scroll_down(ARG(0, 1)); break;
    case 'm': sgr(P, np); break;
    case 'r': {
      int a = ARG(0, 1) - 1, b = ARG(1, trows) - 1;
      if (a < 0) a = 0;
      if (b >= trows) b = trows - 1;
      if (a < b) { t_top = a; t_bot = b; }
      tcx = 0; tcy = 0;
      break;
    }
    case 's': t_scx = tcx; t_scy = tcy; break;
    case 'u': tcx = t_scx; tcy = t_scy; break;
    case 'n':
      if (np && P[0] == 6) { char b[32]; int k = snprintf(b, sizeof b, "\x1b[%d;%dR", tcy + 1, tcx + 1); pty_write(b, k); }
      else if (np && P[0] == 5) pty_write("\x1b[0n", 4);
      break;
    case 'c': pty_write("\x1b[?1;2c", 7); break;
    default: break;
  }
#undef ARG
}

static void term_feed(const unsigned char *b, int n) {
  for (int i = 0; i < n; i++) {
    unsigned char c = b[i];
    switch (ps) {
      case PS_GND:
        if (uneed) {
          if ((c & 0xC0) == 0x80) { ucp = (ucp << 6) | (c & 63); if (--uneed == 0) put_char(ucp); continue; }
          uneed = 0;
        }
        if (c == 0x1b) ps = PS_ESC;
        else if (c == '\r') { tcx = 0; t_wrap = 0; }
        else if (c == '\n' || c == '\v' || c == '\f') { lf(); t_wrap = 0; }
        else if (c == '\b') { if (tcx > 0) tcx--; t_wrap = 0; }
        else if (c == '\t') { tcx = (tcx / 8 + 1) * 8; if (tcx >= tcols) tcx = tcols - 1; }
        else if (c < 32 || c == 127) {}
        else if (c < 0x80) put_char(c);
        else if (c >= 0xF0) { ucp = c & 7; uneed = 3; }
        else if (c >= 0xE0) { ucp = c & 15; uneed = 2; }
        else if (c >= 0xC0) { ucp = c & 31; uneed = 1; }
        break;
      case PS_ESC:
        ps = PS_GND;
        if (c == '[') { ps = PS_CSI; csil = 0; }
        else if (c == ']' || c == 'P' || c == '_' || c == '^') ps = PS_OSC;
        else if (c == '(' || c == ')' || c == '*' || c == '+' || c == '#') ps = PS_SKIP1;
        else if (c == '7') { t_scx = tcx; t_scy = tcy; }
        else if (c == '8') { tcx = t_scx; tcy = t_scy; }
        else if (c == 'M') { if (tcy == t_top) scroll_down(1); else if (tcy > 0) tcy--; }
        else if (c == 'D') lf();
        else if (c == 'E') { tcx = 0; lf(); }
        else if (c == 'c') { pen_reset(); set_alt(0); t_top = 0; t_bot = trows - 1; tcx = tcy = 0; for (int r = 0; r < trows; r++) clear_cells(&CELL(r, 0), 0, tcols); }
        break;
      case PS_CSI:
        if (c >= 0x40 && c <= 0x7e) { csi[csil] = 0; csi_dispatch((char)c); ps = PS_GND; }
        else if (csil < (int)sizeof csi - 1) csi[csil++] = (char)c;
        break;
      case PS_OSC:
        if (c == 7) ps = PS_GND;
        else if (c == 0x1b) ps = PS_OSC_ESC;
        break;
      case PS_OSC_ESC: ps = c == '\\' ? PS_GND : PS_OSC; break;
      case PS_SKIP1: ps = PS_GND; break;
    }
  }
}

static int pty_reader(void *arg) {
  int fdn = (int)(intptr_t)arg;
  char buf[16384];
  for (;;) {
    ssize_t n = read(fdn, buf, sizeof buf);
    if (n < 0 && errno == EINTR) continue;
    SDL_Event e;
    SDL_zero(e);
    e.type = EV_PTY;
    if (n <= 0) { e.user.code = 0; e.user.data1 = NULL; SDL_PushEvent(&e); break; }
    char *d = malloc((size_t)n);
    memcpy(d, buf, (size_t)n);
    e.user.code = (int)n;
    e.user.data1 = d;
    while (SDL_PushEvent(&e) < 0) SDL_Delay(5);
  }
  return 0;
}

static void term_start(void) {
  struct winsize ws = {(unsigned short)trows, (unsigned short)tcols, 0, 0};
  int fdm;
  pid_t pid = forkpty(&fdm, NULL, NULL, &ws);
  if (pid < 0) { const char *m = "Could not start the terminal\r\n";
    term_feed((const unsigned char *)m, (int)strlen(m)); term_dead = 1; return; }
  if (pid == 0) {
    setenv("TERM", "xterm-256color", 1);
    setenv("COLORTERM", "truecolor", 1);
    const char *lang = getenv("LANG");
    if (!lang || !strstr(lang, "UTF-8")) setenv("LANG", "C.UTF-8", 1);
    if (chdir(root) != 0) { if (chdir(getenv("HOME") ? getenv("HOME") : "/") != 0) {} }
    const char *sh = getenv("SHELL");
    if (!sh || !*sh) sh = "/bin/sh";
    execlp(sh, sh, (char *)NULL);
    _exit(127);
  }
  pty_fd = fdm;
  child_pid = pid;
  term_dead = 0;
  SDL_DetachThread(SDL_CreateThread(pty_reader, "pty", (void *)(intptr_t)fdm));
}

static void term_ended(void) {
  if (pty_fd >= 0) close(pty_fd);
  pty_fd = -1;
  if (child_pid > 0) waitpid(child_pid, NULL, WNOHANG);
  child_pid = 0;
  term_dead = 1;
  pen_reset();
  const char *m = "\r\n\x1b[90m[session ended: press Enter to start again]\x1b[0m\r\n";
  term_feed((const unsigned char *)m, (int)strlen(m));
}

static void term_key(SDL_Keysym ks) {
  hview = 0;
  if (term_dead) {
    if (ks.sym == SDLK_RETURN || ks.sym == SDLK_KP_ENTER) term_start();
    return;
  }
  int ctrl = ks.mod & KMOD_CTRL, shift = ks.mod & KMOD_SHIFT, alt = ks.mod & KMOD_ALT;
  if (ctrl && shift && ks.sym == SDLK_v) {
    if (SDL_HasClipboardText()) { char *s = SDL_GetClipboardText(); pty_write(s, strlen(s)); SDL_free(s); }
    return;
  }
  const char *seq = NULL;
  char buf[8];
  switch (ks.sym) {
    case SDLK_RETURN: case SDLK_KP_ENTER: seq = "\r"; break;
    case SDLK_BACKSPACE: seq = ctrl ? "\x17" : "\x7f"; break;
    case SDLK_TAB: seq = shift ? "\x1b[Z" : "\t"; break;
    case SDLK_ESCAPE: seq = "\x1b"; break;
    case SDLK_UP: seq = t_appcur ? "\x1bOA" : "\x1b[A"; break;
    case SDLK_DOWN: seq = t_appcur ? "\x1bOB" : "\x1b[B"; break;
    case SDLK_RIGHT: seq = ctrl ? "\x1b[1;5C" : t_appcur ? "\x1bOC" : "\x1b[C"; break;
    case SDLK_LEFT: seq = ctrl ? "\x1b[1;5D" : t_appcur ? "\x1bOD" : "\x1b[D"; break;
    case SDLK_HOME: seq = "\x1b[H"; break;
    case SDLK_END: seq = "\x1b[F"; break;
    case SDLK_DELETE: seq = "\x1b[3~"; break;
    case SDLK_INSERT: seq = "\x1b[2~"; break;
    case SDLK_PAGEUP: seq = "\x1b[5~"; break;
    case SDLK_PAGEDOWN: seq = "\x1b[6~"; break;
    default:
      if (ctrl && ks.sym >= SDLK_a && ks.sym <= SDLK_z) { buf[0] = (char)(ks.sym - SDLK_a + 1); buf[1] = 0; seq = buf; }
      else if (ctrl && ks.sym == SDLK_LEFTBRACKET) seq = "\x1b";
      else if (ctrl && ks.sym == SDLK_BACKSLASH) seq = "\x1c";
      else if (ctrl && ks.sym == SDLK_SPACE) { pty_write("", 1); return; }
      else if (alt && ks.sym >= 32 && ks.sym < 127) { buf[0] = 0x1b; buf[1] = (char)ks.sym; buf[2] = 0; seq = buf; }
      break;
  }
  if (seq) pty_write(seq, strlen(seq));
}


/* ================= геометрія ================= */
static float side_w, term_h, tabs_h;
static float g_docx, g_docy, g_docw, g_doch, g_pagex;
static float g_termx, g_termy, g_termw, g_termh;
static double zoom_s;
static int doc_drag, sb_drag;
static float sb_off;
static Uint32 ignore_until;
static int ptr_out;            /* вказівник за межами вікна */
static int norm_w = 1440, norm_h = 940; /* розмір вікна, коли воно не розгорнуте */
static int drag_detached;      /* виділення триває, хоча система вже «відпустила» кнопку */
static Uint32 release_pending; /* коли прийшло відпускання кнопки, ще не підтверджене */
static float last_in_y;        /* остання позиція миші всередині вікна */
static int dbg;                /* MASHYNKA_DEBUG=1 — друкувати події миші */
static int drv_x11;            /* працюємо під X11 / XWayland */
static float last_mx, last_my; /* остання відома позиція миші (будь-де) */

/* Журнал останнього виділення мишкою: ~/.local/share/mashynka/mouse.log.
   Якщо виділення за вікном знову зупиниться — цей файл покаже, що надсилає система. */
static FILE *mlog;
static void data_dir(char *out, size_t n);
static void mkdirs(const char *path);
#define MLOG(...) do { if (mlog) { fprintf(mlog, __VA_ARGS__); fflush(mlog); } if (dbg) fprintf(stderr, __VA_ARGS__); } while (0)
static void mlog_open(void) {
  char d[1100], p[1200];
  data_dir(d, sizeof d);
  mkdirs(d);
  snprintf(p, sizeof p, "%s/mouse.log", d);
  if (mlog) fclose(mlog);
  mlog = fopen(p, "w");
  MLOG("driver %s, window %dx%d, pr %.2f\n", SDL_GetCurrentVideoDriver(), ow, oh, pr);
}

static void end_drag(void) {
  if (doc_drag) SDL_CaptureMouse(SDL_FALSE);
  if (doc_drag) MLOG("end of selection\n");
  if (mlog) { fclose(mlog); mlog = NULL; }
  doc_drag = 0; drag_detached = 0; release_pending = 0;
  M.down = 0; M.released = 1;
}

static int near_edge(float x, float y) { float e = 12 * (float)u; return x < e || y < e || x > ow - e || y > oh - e; }

/* Коли мишка за вікном і події руху не приходять, ведемо виділення
   в той бік, куди вона вийшла. */
static void drag_track(void) {
  if (drag_detached && drv_x11 && !(SDL_GetGlobalMouseState(NULL, NULL) & SDL_BUTTON_LMASK)) { end_drag(); return; }
  if (!(ptr_out || drag_detached)) return;
  if (last_in_y < oh * 0.3f) M.y = g_docy - 40 * (float)u;
  else if (last_in_y > oh * 0.7f) M.y = g_docy + g_doch + 100 * (float)u;
}

static double line_top_pt(int l) { return GAP_PT + (l / TPP) * (PAGE_H + GAP_PT) + MARGIN_TOP + (HDR + l % TPP) * LEAD_PT; }

static int pos_from_xy(double x, double y) {
  double s = zoom_s, dy = (y - g_docy + D->scroll) / s;
  int np = npages();
  int p = (int)floor((dy - GAP_PT) / (PAGE_H + GAP_PT));
  if (p < 0) p = 0;
  if (p >= np) p = np - 1;
  double in = dy - (GAP_PT + p * (PAGE_H + GAP_PT));
  int r = (int)floor((in - MARGIN_TOP) / LEAD_PT) - HDR;
  if (r < 0) { if (p == 0) return 0; r = 0; }
  if (r >= TPP) r = TPP - 1;
  int l = p * TPP + r;
  if (l >= D->nL) return D->n;
  int col = (int)lround(((x - g_pagex) / s - MARGIN_X) / ADVPT);
  if (col < 0) col = 0;
  if (col > maxcol(l)) col = maxcol(l);
  return D->L[l].start + col;
}

static void move_to(int p, int shift) {
  if (p < 0) p = 0;
  if (p > D->n) p = D->n;
  D->cur = p;
  if (!shift) D->anchor = p;
  need_visible = 1;
  blink_t = SDL_GetTicks();
}

static void move_vert(int dl, int shift) {
  int l = line_of(D->cur), col = D->cur - D->L[l].start;
  if (D->desired_col < 0) D->desired_col = col;
  int nl = l + dl, keep = D->desired_col;
  if (nl < 0) move_to(0, shift);
  else if (nl >= D->nL) move_to(D->n, shift);
  else move_to(D->L[nl].start + (keep < maxcol(nl) ? keep : maxcol(nl)), shift);
  D->desired_col = keep;
}

static int in_rect(float x, float y, float rx, float ry, float rw, float rh) { return x >= rx && x < rx + rw && y >= ry && y < ry + rh; }
static int in_doc(float x, float y) { return in_rect(x, y, g_docx, g_docy, g_docw - 14 * (float)u, g_doch); }

static void doc_press(float x, float y, int clicks, int shift) {
  int p = pos_from_xy(x, y);
  if (clicks == 2) { D->anchor = word_left(p < D->n && is_word(D->c[p]) ? p + 1 : p); D->cur = word_right(p); }
  else if (clicks >= 3) { int a = lstart(p), b = lend(p); D->anchor = a; D->cur = b < D->n ? b + 1 : b; }
  else { move_to(p, shift); if (!shift) tbl_snap(); }
  D->desired_col = -1;
  doc_drag = clicks == 1;
  if (doc_drag) { SDL_CaptureMouse(SDL_TRUE); mlog_open(); MLOG("pressed %.0f %.0f\n", x, y); } /* рух миші приходить і за межами вікна */
  blink_t = SDL_GetTicks();
}

static void copy_sel(void) {
  if (D->cur == D->anchor) return;
  char *s = text_utf8(D->c, sel_a(), sel_b(), NULL);
  SDL_SetClipboardText(s);
  free(s);
}

static void paste(void) {
  if (!SDL_HasClipboardText()) return;
  char *s = SDL_GetClipboardText();
  int n;
  uint32_t *cp = utf8_dec(s, strlen(s), &n);
  SDL_free(s);
  if (n && !ro_block() && !tbl_type(cp, n)) insert(cp, n, 0);
  free(cp);
}

static void cycle_style(void) {
  for (int i = 1; i <= 3; i++) {
    int st = (S.style + i) % 3;
    if (load_doc_font(st)) { save_settings(); char m[64]; snprintf(m, sizeof m, "Ribbon %s", STYLE_LBL[st]); set_toast(m); return; }
  }
}

/* ================= масштаб ================= */
static void update_dims(void);
static double real_scale(void);
static int cur_zoom_pct(void) { return (int)lround(zoom_s / real_scale() * 100); }

/* Калібрування: прикладіть лінійку до екрана й підганяйте, доки ширина аркуша не стане 21 см. */
static void calib_step(int dir) {
  S.calib += dir;
  if (S.calib < 30) S.calib = 30;
  if (S.calib > 400) S.calib = 400;
  if (S.zoom != 100) S.zoom = 100;
  save_settings();
  update_dims();
  char m[128];
  if (zoom_s < real_scale() - 1e-9) snprintf(m, sizeof m, "Calibration %d%%: the page no longer fits the width", S.calib);
  else snprintf(m, sizeof m, "Calibration %d%%: the page should be 21 cm wide", S.calib);
  set_toast(m);
}

static void zoom_toast(void) {
  save_settings();
  update_dims();
  char m[64];
  if (S.zoom == 0) snprintf(m, sizeof m, "Fit width");
  else snprintf(m, sizeof m, "Zoom %d%%", cur_zoom_pct());
  set_toast(m);
}

static void zoom_step(int dir) {
  int z = S.zoom ? S.zoom : cur_zoom_pct();
  int k = 0;
  if (dir > 0) { while (k < NZOOMS && ZOOMS[k] <= z) k++; if (k >= NZOOMS) k = NZOOMS - 1; }
  else { k = NZOOMS - 1; while (k > 0 && ZOOMS[k] >= z) k--; }
  S.zoom = ZOOMS[k];
  zoom_toast();
}

/* ================= клавіатура ================= */
enum { SEC_FILE, SEC_FIND, SEC_INSERT, SEC_PDF, SEC_SHEET, SEC_PAGES, SEC_DOC, SEC_KEYS, NSEC };
static void panel_goto(int sec);

static int global_key(SDL_Keysym ks) {
  if (mods_of(ks.mod) == MD_CTRL && ks.scancode >= SDL_SCANCODE_1 && ks.scancode <= SDL_SCANCODE_9) {
    int i = ks.scancode - SDL_SCANCODE_1;
    if (ndocs) { activate(i < ndocs ? i : ndocs - 1); focus = F_EDIT; }
    return 1;
  }
  if (hit(A_NEW, ks)) { new_tab(); focus = F_EDIT; return 1; }
  if (hit(A_OPEN, ks)) { focus = F_FILES; ffocus = NULL; if (tree_sel < 0 && nnodes) tree_sel = 0; return 1; }
  if (hit(A_QUIT, ks)) { quit_step(); return 1; }
  if (hit(A_SETTINGS, ks)) { panel_goto(SEC_SHEET); return 1; }
  if (hit(A_STYLE, ks)) { cycle_style(); return 1; }
  if (hit(A_ZOOM_IN, ks)) { zoom_step(1); return 1; }
  if (hit(A_ZOOM_OUT, ks)) { zoom_step(-1); return 1; }
  if (hit(A_ZOOM_RESET, ks)) { S.zoom = S.zoom == 100 ? 0 : 100; zoom_toast(); return 1; }
  if (hit(A_CALIB_UP, ks)) { calib_step(1); return 1; }
  if (hit(A_CALIB_DOWN, ks)) { calib_step(-1); return 1; }
  if (hit(A_SLIDES, ks)) {
    const char *dir = tree_sel >= 0 && tree_sel < nnodes && nodes[tree_sel].dir && focus == F_FILES ? nodes[tree_sel].path : root;
    open_slides(dir);
    focus = F_EDIT;
    return 1;
  }
  if (hit(A_TAB_NEXT, ks)) { if (ndocs) { activate((act + 1) % ndocs); focus = F_EDIT; } return 1; }
  if (hit(A_TAB_PREV, ks)) { if (ndocs) { activate((act - 1 + ndocs) % ndocs); focus = F_EDIT; } return 1; }
  if (!D) return 0;
  if (hit(A_INSERT, ks)) { panel_goto(SEC_INSERT); return 1; }
  if (hit(A_SAVE, ks)) { act_save(); return 1; }
  if (hit(A_SAVE_AS, ks)) { name_focus(); return 1; }
  if (hit(A_PDF, ks)) { act_pdf(); return 1; }
  if (hit(A_CLOSE, ks)) { ask_close(act); return 1; }
  if (hit(A_FIND, ks)) { find_open(0); focus = F_EDIT; return 1; }
  if (hit(A_REPLACE, ks)) { find_open(1); focus = F_EDIT; return 1; }
  if (hit(A_FIND_NEXT, ks)) { if (!F.q.s[0]) find_open(0); else find_step(1); return 1; }
  if (hit(A_FIND_PREV, ks)) { if (!F.q.s[0]) find_open(0); else find_step(-1); return 1; }
  if (hit(A_VIEW, ks)) {
    if (D->ro && D->view != VIEW_SLIDES) { /* перебираємо вигляди, які є для цього типу файлу */
      int ft = D->ftype, cyc[4], nc = 0;
      if (ft == FT_PDF) { cyc[nc++] = VIEW_PDF; cyc[nc++] = VIEW_EXTRACT; }
      else if (ft == FT_IMAGE) { cyc[nc++] = VIEW_IMAGE; cyc[nc++] = VIEW_EXTRACT; }
      else if (ft == FT_ELF) cyc[nc++] = VIEW_ASM;
      else if (ft == FT_ARCHIVE || ft == FT_DOCX || ft == FT_ODT || ft == FT_EPUB || ft == FT_MEDIA) cyc[nc++] = VIEW_EXTRACT;
      cyc[nc++] = VIEW_HEX;
      int k = 0;
      for (int i = 0; i < nc; i++) if (cyc[i] == D->view) k = i;
      int v = load_view(D, D->path, cyc[(k + 1) % nc]);
      static const char *vn[] = {"text", "hex", "assembly", "text", "PDF pages", "image", "slideshow"};
      char m[100];
      snprintf(m, sizeof m, "View: %s", vn[v]);
      set_toast(m);
    }
    return 1;
  }
  return 0;
}

static void doc_key(SDL_Keysym ks) {
  if (global_key(ks)) return;
  if (!D) return;
  if (media_key(ks)) return;
  if (is_media_view(D->view)) return;
  for (int a = A_ROW_BELOW; a <= A_COL_NARROWER; a++) if (hit(a, ks)) { tbl_op(a); return; }
  if (!D->ro && tbl_key(ks)) return;
  int cmd = ks.mod & CMDMOD, shift = ks.mod & KMOD_SHIFT;
  int has_sel = D->cur != D->anchor;
#ifdef __APPLE__
  int wordmod = ks.mod & KMOD_ALT;
#else
  int wordmod = cmd;
#endif
  if (hit(A_UNDO, ks)) { undo(); return; }
  if (hit(A_REDO, ks)) { redo(); return; }
  if (hit(A_ALL, ks)) { D->anchor = 0; D->cur = D->n; return; }
  if (hit(A_COPY, ks)) { copy_sel(); return; }
  if (hit(A_CUT, ks)) {
    if (ro_block()) return;
    if (has_sel && tbl_erase(0, 0)) { copy_sel(); return; }
    if (has_sel) { copy_sel(); erase(0, 0); }
    else { int a = lstart(D->cur), b = lend(D->cur); D->anchor = a; D->cur = b < D->n ? b + 1 : b; copy_sel(); erase(0, 0); }
    return;
  }
  if (hit(A_PASTE, ks)) { paste(); return; }
  if (hit(A_DUP, ks)) { if (!tbl_line_op(A_DUP)) lines_duplicate(); return; }
  if (hit(A_DEL_LINE, ks)) { if (!tbl_line_op(A_DEL_LINE)) lines_delete(); return; }
  if (hit(A_MOVE_UP, ks)) { if (!tbl_line_op(A_MOVE_UP)) lines_move(-1); return; }
  if (hit(A_MOVE_DOWN, ks)) { if (!tbl_line_op(A_MOVE_DOWN)) lines_move(1); return; }
  if (hit(A_SEL_LINE, ks)) { select_lines(); return; }
  if (hit(A_INDENT, ks)) { lines_indent(1); return; }
  if (hit(A_OUTDENT, ks)) { lines_indent(-1); return; }
  if (hit(A_DATE, ks)) { insert_date(S.datefmt); return; }
  if (hit(A_TABLE_FMT, ks)) { tbl_from_text(); return; }
  if (hit(A_CENTER, ks)) { format_lines(FMT_CENTER); return; }
  if (hit(A_TITLE, ks)) { format_lines(FMT_TITLE); return; }
  if (cmd) {
    switch (ks.sym) {
      case SDLK_HOME: move_to(0, shift); D->desired_col = -1; return;
      case SDLK_END: move_to(D->n, shift); D->desired_col = -1; return;
#ifdef __APPLE__
      case SDLK_UP: move_to(0, shift); D->desired_col = -1; return;
      case SDLK_DOWN: move_to(D->n, shift); D->desired_col = -1; return;
      case SDLK_LEFT: move_to(D->L[line_of(D->cur)].start, shift); D->desired_col = -1; return;
      case SDLK_RIGHT: { int l = line_of(D->cur); move_to(D->L[l].start + maxcol(l), shift); D->desired_col = -1; return; }
#endif
      default: break;
    }
  }
  switch (ks.sym) {
    case SDLK_LEFT:
      if (has_sel && !shift) move_to(sel_a(), 0);
      else move_to(wordmod ? word_left(D->cur) : D->cur - 1, shift);
      D->desired_col = -1;
      break;
    case SDLK_RIGHT:
      if (has_sel && !shift) move_to(sel_b(), 0);
      else move_to(wordmod ? word_right(D->cur) : D->cur + 1, shift);
      D->desired_col = -1;
      break;
    case SDLK_UP: move_vert(-1, shift); if (!shift) tbl_snap(); break;
    case SDLK_DOWN: move_vert(1, shift); if (!shift) tbl_snap(); break;
    case SDLK_PAGEUP: move_vert(-(int)(g_doch / (LEAD_PT * zoom_s)) + 2, shift); break;
    case SDLK_PAGEDOWN: move_vert((int)(g_doch / (LEAD_PT * zoom_s)) - 2, shift); break;
    case SDLK_HOME: {
      int l = line_of(D->cur), st = D->L[l].start, fe = st;
      while (fe < D->n && D->c[fe] == ' ' && fe < st + D->L[l].len) fe++;
      move_to(D->cur == fe ? st : fe, shift);
      D->desired_col = -1;
      break;
    }
    case SDLK_END: { int l = line_of(D->cur); move_to(D->L[l].start + maxcol(l), shift); D->desired_col = -1; break; }
    case SDLK_BACKSPACE: if (!ro_block() && !tbl_erase(0, wordmod)) { smart_pre(); erase(0, wordmod); smart_post(); } break;
    case SDLK_DELETE: if (!ro_block() && !tbl_erase(1, wordmod)) { smart_pre(); erase(1, wordmod); smart_post(); } break;
    case SDLK_RETURN: case SDLK_KP_ENTER: { if (ro_block() || smart_enter()) break; uint32_t nl = '\n'; insert(&nl, 1, 0); break; }
    case SDLK_TAB: {
      int multi = 0;
      for (int i = sel_a(); i < sel_b(); i++) if (D->c[i] == '\n') { multi = 1; break; }
      if (shift || multi) lines_indent(shift ? -1 : 1);
      else {
        int n = TAB_STOP - (D->cur - D->L[line_of(D->cur)].start) % TAB_STOP;
        uint32_t sp[TAB_STOP];
        for (int i = 0; i < n; i++) sp[i] = ' ';
        insert(sp, n, 0);
      }
      break;
    }
    case SDLK_ESCAPE:
      if (F.open) { F.open = 0; ffocus = NULL; }
      else if (has_sel) move_to(D->cur, 0);
      break;
    default: break;
  }
}


static void tree_ensure_visible(void) {
  double rh = 26 * u, top = tree_sel * rh, viewh = oh - 52 * u;
  if (top < tree_scroll) tree_scroll = top;
  if (top + rh > tree_scroll + viewh) tree_scroll = top + rh - viewh;
}

static void files_key(SDL_Keysym ks) {
  if (global_key(ks)) return;
  if (hit(A_TRASH, ks)) { tree_trash(); return; }
  if (hit(A_TRASH_UNDO, ks)) { trash_undo(); return; }
  if (hit(A_TREE_UP, ks)) { tree_up(); return; }
  switch (ks.sym) {
    case SDLK_UP: if (tree_sel > 0) tree_sel--; else if (nnodes) tree_sel = 0; tree_ensure_visible(); break;
    case SDLK_DOWN: if (tree_sel < nnodes - 1) tree_sel++; tree_ensure_visible(); break;
    case SDLK_RETURN: case SDLK_KP_ENTER:
      if (tree_sel >= 0 && nodes[tree_sel].dir) tree_enter(tree_sel);
      else { tree_activate(tree_sel); if (tree_sel >= 0) focus = F_EDIT; }
      break;
    case SDLK_RIGHT:
      if (tree_sel >= 0 && nodes[tree_sel].dir && find_expanded(nodes[tree_sel].path) < 0) { toggle_expanded(nodes[tree_sel].path); tree_rebuild(); }
      break;
    case SDLK_LEFT:
      if (tree_sel >= 0 && nodes[tree_sel].dir && find_expanded(nodes[tree_sel].path) >= 0) { toggle_expanded(nodes[tree_sel].path); tree_rebuild(); }
      else if (tree_sel >= 0) { int d = nodes[tree_sel].depth; for (int i = tree_sel - 1; i >= 0; i--) if (nodes[i].depth < d) { tree_sel = i; break; } }
      break;
    case SDLK_ESCAPE: focus = F_EDIT; break;
    default: break;
  }
}


/* Малює картинку, слайдшоу або сторінки PDF у області документа. */
static void draw_media(float x0, float y0, float w, float h) {
  Media *m = D->media;
  float fu = (float)u;
  fill_rect(x0, y0, w, h, C_DESK, 255);
  if (!m) return;
  clip(x0, y0, w, h);
  if (D->view == VIEW_PDF) {
    double s = zoom_s, gap = GAP_PT;
    double pwp = m->pw * s, php = m->ph * s;
    double total = (m->pages * (m->ph + gap) + gap) * s;
    double maxs = total - h;
    if (D->scroll > maxs) D->scroll = maxs;
    if (D->scroll < 0) D->scroll = 0;
    float px = (float)(x0 + (w - pwp) / 2);
    int rendering = 0;
    for (int p = 0; p < m->pages; p++) if (m->pstate[p] == 1) rendering++;
    for (int p = 0; p < m->pages; p++) {
      float top = (float)(y0 + (gap + p * (m->ph + gap)) * s - D->scroll);
      if (top > y0 + h + php || top + php < y0 - php) { /* далеко від екрана — звільняємо пам'ять */
        if (m->pt[p] && SDL_GetTicks() - m->pused[p] > 4000) { SDL_DestroyTexture(m->pt[p]); m->pt[p] = NULL; m->pstate[p] = 0; }
        continue;
      }
      if (m->pstate[p] == 0 && rendering < 2) { m->pstate[p] = 1; rendering++; job_start(D, D->path, p); }
      SDL_FRect dr = {px, top, (float)pwp, (float)php};
      if (m->pt[p]) { SDL_RenderCopyF(R, m->pt[p], NULL, &dr); m->pused[p] = SDL_GetTicks(); }
      else fill_rect(px, top, (float)pwp, (float)php, (RGB){250, 249, 246}, 255);
    }
    if (total > h) {
      float tw = 6 * fu, tx = x0 + w - tw - 4 * fu, th = (float)fmax(40 * u, h * h / total), ty = (float)(y0 + (h - th) * D->scroll / maxs);
      fill_rrect(tx, ty, tw, th, tw / 2, (RGB){255, 255, 255}, 28);
    }
  } else {
    if (D->view == VIEW_SLIDES && !m->paused && !m->waiting && SDL_GetTicks() >= m->next_t) slide_next(D); /* і коли тека порожня — раптом щось з'явиться */
    if (m->tex) {
      float pad = 16 * fu, aw = w - 2 * pad, ah = h - 2 * pad - (D->view == VIEW_SLIDES ? 22 * fu : 0);
      float sc = fminf(aw / m->tw, ah / m->th);
      if (sc > 1 && D->view == VIEW_IMAGE) sc = fminf(sc, 1.0f * (float)pr); /* маленькі картинки не роздуваємо */
      float dw = m->tw * sc, dh = m->th * sc;
      SDL_FRect dr = {x0 + (w - dw) / 2, y0 + pad + (ah - dh) / 2, dw, dh};
      SDL_RenderCopyF(R, m->tex, NULL, &dr);
    }
    if (m->err[0]) text(&c_ui, m->err, x0 + w / 2, y0 + h / 2, C_DIM, 1);
    if (D->view == VIEW_SLIDES && m->shown[0]) {
      char cap[400];
      snprintf(cap, sizeof cap, "%s   ·   %d in the folder%s", basename_of(m->shown), m->nf, m->paused ? "   ·   paused" : "");
      text(&c_small, cap, x0 + w / 2, y0 + h - 12 * fu, C_FAINT, 1);
    }
  }
  SDL_RenderSetClipRect(R, NULL);
}

/* Клавіші у вигляді картинок: стрілки — далі/назад, пробіл — пауза; у PDF — прокрутка. */
static int media_key(SDL_Keysym ks) {
  if (!D || !is_media_view(D->view) || !D->media) return 0;
  Media *m = D->media;
  if (D->view == VIEW_SLIDES) {
    if (ks.sym == SDLK_RIGHT || ks.sym == SDLK_DOWN || ks.sym == SDLK_PAGEDOWN) { slide_next(D); return 1; }
    if (ks.sym == SDLK_LEFT || ks.sym == SDLK_UP || ks.sym == SDLK_PAGEUP) { slide_prev(D); return 1; }
    if (ks.sym == SDLK_SPACE) { m->paused = !m->paused; m->next_t = SDL_GetTicks() + 5000; set_toast(m->paused ? "Slideshow paused · Space continues" : "Slideshow: a new picture every 5 seconds"); return 1; }
  } else if (D->view == VIEW_PDF) {
    double step = 60 * u, page = g_doch - 40 * u;
    switch (ks.sym) {
      case SDLK_DOWN: D->scroll += step; return 1;
      case SDLK_UP: D->scroll -= step; return 1;
      case SDLK_PAGEDOWN: case SDLK_SPACE: D->scroll += page; return 1;
      case SDLK_PAGEUP: D->scroll -= page; return 1;
      case SDLK_HOME: D->scroll = 0; return 1;
      case SDLK_END: D->scroll = 1e12; return 1;
      default: break;
    }
  }
  return 0;
}

/* ================= малювання: аркуш ================= */
static void draw_doc(void) {
  float x0 = g_docx, y0 = g_docy, w = g_docw, h = g_doch;
  if (font_missing) { /* без шрифту TT2020 писати нічим — кажемо про це просто на столі */
    fill_rect(x0, y0, w, h, C_DESK, 255);
    text(&c_uib, "The TT2020 font is missing", x0 + w / 2, y0 + h / 2 - 14 * (float)u, C_TXT, 1);
    text(&c_ui, "Put TT2020Style*-Regular.ttf in a fonts folder next to the program. Esc quits.", x0 + w / 2, y0 + h / 2 + 14 * (float)u, C_DIM, 1);
    return;
  }
  if (!D) { fill_rect(x0, y0, w, h, C_DESK, 255); return; } /* немає вкладок — порожній стіл */
  if (is_media_view(D->view)) { draw_media(x0, y0, w, h); return; }
  double s = zoom_s;
  int np = npages();
  double doch = (np * (PAGE_H + GAP_PT) + GAP_PT) * s;
  double pagew = PAGE_W * s;

  /* тягнемо виділення за край — аркуш прокручується сам */
  if (doc_drag && M.down && !modal) drag_track();
  if (doc_drag && M.down && !modal) {
    /* Зона автопрокрутки починається трохи всередині аркуша, бо над ним мало місця
       (лише рядок вкладок і край екрана), а під ним — термінал. */
    double zone = 28 * u, d = 0;
    if (M.y < y0 + zone) d = -((y0 + zone) - M.y);
    else if (M.y > y0 + h - zone) d = M.y - (y0 + h - zone);
    if (d != 0) {
      double ad = fabs(d);
      double sp = ad * (1.0 + ad / (6 * u)) * 0.5 + 6 * u; /* чим далі за край, тим швидше */
      D->scroll += d < 0 ? -sp : sp;
    }
    float yy = M.y < y0 + 1 ? y0 + 1 : M.y > y0 + h - 1 ? y0 + h - 1 : M.y;
    double maxs0 = doch - h;
    if (D->scroll > maxs0) D->scroll = maxs0;
    if (D->scroll < 0) D->scroll = 0;
    D->cur = pos_from_xy(M.x, yy);
    D->desired_col = -1;
    need_visible = 0;
  }
  int cl = line_of(D->cur);
  if (need_visible) {
    double top = line_top_pt(cl) * s, bot = top + LEAD_PT * s, m = LEAD_PT * s * 2;
    if (top - m < D->scroll) D->scroll = top - m;
    if (bot + m > D->scroll + h) D->scroll = bot + m - h;
    need_visible = 0;
  }
  double maxs = doch - h;
  if (D->scroll > maxs) D->scroll = maxs;
  if (D->scroll < 0) D->scroll = 0;

  float px = (float)(x0 + (w - pagew) / 2);
  g_pagex = px;
  gcache_size(&c_doc, FONT_PT * s);
  if (F.open) find_update();

  clip(x0, y0, w, h);
  fill_rect(x0, y0, w, h, C_DESK, 255);
  RGB paper = PAPERS[S.paper], ink = INKS[S.ink];
  int a = sel_a(), b = sel_b();
  double lineh = FONT_PT * 7.0 / 6.0, off = (LEAD_PT - lineh) / 2;
  for (int p = 0; p < np; p++) {
    float top = (float)(y0 + (GAP_PT + p * (PAGE_H + GAP_PT)) * s - D->scroll);
    float ph = (float)(PAGE_H * s);
    if (top > y0 + h || top + ph < y0) continue;
    SDL_Texture *ptex = S.paper == 2 ? paper_for(p) : NULL;
    if (ptex) { SDL_FRect dr = {px, top, (float)pagew, ph}; SDL_RenderCopyF(R, ptex, NULL, &dr); }
    else fill_rect(px, top, (float)pagew, ph, paper, 255);
    for (int which = 0; which < 2; which++) {
      if (which == 0 ? !HDR : !FTR) continue;
      char dt[256];
      deco_text(p, which, dt, sizeof dt);
      int dn;
      uint32_t *dc = utf8_dec(dt, strlen(dt), &dn);
      int row = which == 0 ? 0 : LPP - 1, col = which == 0 ? 0 : COLS - dn;
      if (col < 0) col = 0;
      float ly = (float)(top + (MARGIN_TOP + row * LEAD_PT) * s);
      /* службові рядки не редагуються, але виглядають точно як увесь текст */
      draw_mono(dc, dn, (float)(px + (MARGIN_X + col * ADVPT) * s), (float)(ly + (off + BASEPT) * s), (float)(ADVPT * s), ink);
      free(dc);
    }
    for (int r = 0; r < TPP; r++) {
      int l = p * TPP + r;
      if (l >= D->nL) break;
      float ly = (float)(top + (MARGIN_TOP + (r + HDR) * LEAD_PT) * s);
      if (ly > y0 + h || ly + LEAD_PT * s < y0) continue;
      int st = D->L[l].start, en = st + D->L[l].len;
      if (F.open && F.nm && F.qn) {
        int k = lower_bound(F.m, F.nm, st - F.qn + 1);
        for (; k < F.nm && F.m[k] < en; k++) {
          int ms = F.m[k] > st ? F.m[k] : st, me = F.m[k] + F.qn < en ? F.m[k] + F.qn : en;
          if (me > ms) fill_rect((float)(px + (MARGIN_X + (ms - st) * ADVPT) * s), ly, (float)((me - ms) * ADVPT * s), (float)(LEAD_PT * s), k == F.idx ? (RGB){255, 160, 40} : (RGB){255, 214, 120}, k == F.idx ? 150 : 90);
        }
      }
      if (a != b && a <= en && b > st) {
        int c0 = (a > st ? a : st) - st, c1 = (b < en ? b : en) - st;
        if (b > en && en < D->n && D->c[en] == '\n') c1++;
        if (c1 > c0) fill_rect((float)(px + (MARGIN_X + c0 * ADVPT) * s), ly, (float)((c1 - c0) * ADVPT * s), (float)(LEAD_PT * s), C_SEL, 120);
      }
      draw_mono(D->c + st, D->L[l].len, (float)(px + MARGIN_X * s), (float)(ly + (off + BASEPT) * s), (float)(ADVPT * s), ink);
    }
  }
  if (!modal && focus == F_EDIT && !(F.open && ffocus) && ((SDL_GetTicks() - blink_t) / 530) % 2 == 0) {
    double y = y0 + line_top_pt(cl) * s - D->scroll;
    fill_rect((float)(px + (MARGIN_X + (D->cur - D->L[cl].start) * ADVPT) * s - u), (float)(y + (off + lineh * 0.06) * s),
              (float)fmax(2, 2 * u), (float)(lineh * 0.9 * s), C_CARET, 255);
  }

  if (doch > h) {
    float tw = 6 * (float)u, tx = x0 + w - tw - 4 * (float)u;
    float th = (float)fmax(40 * u, h * h / doch), ty = (float)(y0 + (h - th) * D->scroll / maxs);
    int hov = layer == top_layer && inside(tx - 6 * (float)u, y0, tw + 10 * (float)u, h);
    if (hov && M.pressed) { sb_drag = 1; sb_off = (M.y >= ty && M.y < ty + th) ? M.y - ty : th / 2; }
    if (sb_drag && M.down) {
      D->scroll = (M.y - sb_off - y0) / (h - th) * maxs;
      if (D->scroll < 0) D->scroll = 0;
      if (D->scroll > maxs) D->scroll = maxs;
      ty = (float)(y0 + (h - th) * D->scroll / maxs);
    }
    if (!M.down) sb_drag = 0;
    fill_rrect(tx, ty, tw, th, tw / 2, (RGB){255, 255, 255}, hov || sb_drag ? 70 : 28);
  }
  SDL_RenderSetClipRect(R, NULL);
}

/* ================= вкладки ================= */
static float tab_w(void) {
  float avail = g_docw, tw = avail / (ndocs ? ndocs : 1);
  float mx = 220 * (float)u, mn = 90 * (float)u;
  return tw > mx ? mx : tw < mn ? mn : tw;
}

static int tab_at(float x, float y) {
  if (y < 0 || y >= tabs_h || x < side_w || x >= side_w + g_docw) return -1;
  int i = (int)((x - side_w) / tab_w());
  return i < ndocs ? i : -1;
}

static void draw_tabs(void) {
  float fu = (float)u, x0 = side_w, tw = tab_w(), h = tabs_h;
  fill_rect(x0, 0, g_docw, h, C_PANEL, 255);
  clip(x0, 0, g_docw, h);
  int close_i = -1, act_i = -1;
  for (int i = 0; i < ndocs; i++) {
    float x = x0 + i * tw;
    if (x > x0 + g_docw) break;
    int hov = layer == top_layer && inside(x, 0, tw, h);
    int on = i == act;
    if (on) fill_rect(x, 0, tw, h, C_DESK, 255);
    else if (hov) fill_rect(x, 0, tw, h, C_PANEL2, 255);
    if (on) fill_rect(x, 0, tw, 2 * fu, C_ACC, 255);
    if (i > 0 && !on && i - 1 != act) fill_rect(x, 10 * fu, hair(), h - 20 * fu, C_LINE, 255);
    char nm[300], fit[300];
    doc_name(docs[i], nm, sizeof nm);
    if (docs[i]->ro) {
      static const char *suf[] = {"", " · hex", " · asm", " · text", " · pdf", "", " · slides"};
      strncat(nm, suf[docs[i]->view], sizeof nm - strlen(nm) - 1);
    }
    fit_right(&c_ui, nm, tw - 46 * fu, fit, sizeof fit);
    text(&c_ui, fit, x + 14 * fu, mid(&c_ui, 0, h), on ? C_TXT : C_DIM, 0);
    float cx = x + tw - 18 * fu, cy = h / 2, cs = 18 * fu;
    int chov = layer == top_layer && inside(cx - cs / 2, cy - cs / 2, cs, cs);
    if (hov || on) {
      if (chov) fill_rrect(cx - cs / 2, cy - cs / 2, cs, cs, 4 * fu, C_HOVER, 255);
      if (docs[i]->dirty && !chov) fill_rrect(cx - 3.5f * fu, cy - 3.5f * fu, 7 * fu, 7 * fu, 3.5f * fu, on ? C_TXT : C_DIM, 255);
      else draw_x(cx, cy, 7 * fu, chov ? C_TXT : C_DIM);
    } else if (docs[i]->dirty) fill_rrect(cx - 3.5f * fu, cy - 3.5f * fu, 7 * fu, 7 * fu, 3.5f * fu, C_DIM, 255);
    if (layer == top_layer && M.pressed && chov) close_i = i;
    else if (layer == top_layer && M.pressed && hov) act_i = i;
  }
  SDL_RenderSetClipRect(R, NULL);
  fill_rect(x0, h - hair(), g_docw, hair(), C_LINE, 255);
  if (ndocs && act >= 0) { float x = x0 + act * tw; fill_rect(x, h - hair(), tw, hair(), C_DESK, 255); }
  if (close_i >= 0) ask_close(close_i);
  else if (act_i >= 0) { activate(act_i); focus = F_EDIT; }
}

/* ================= дерево ================= */
static void draw_icon(float x, float y, float s, int isdir, RGB c) {
  if (isdir) {
    fill_rrect(x, y + s * 0.12f, s * 0.42f, s * 0.22f, s * 0.08f, c, 255);
    fill_rrect(x, y + s * 0.24f, s, s * 0.64f, s * 0.12f, c, 255);
  } else {
    fill_rrect(x + s * 0.14f, y, s * 0.72f, s * 0.92f, s * 0.12f, c, 255);
    for (int i = 0; i < 3; i++) fill_rect(x + s * 0.28f, y + s * (0.26f + i * 0.18f), s * 0.44f, fmaxf(1, s * 0.07f), C_PANEL, 255);
  }
}

static void draw_tree(void) {
  float th = (float)oh - term_h; /* під провідником — термінал на всю ширину */
  float fu = (float)u, w = side_w, hh = tabs_h, top = hh + 6 * fu, h = th - top;
  fill_rect(0, 0, w, th, C_PANEL, 255);
  fill_rect(w - hair(), 0, hair(), th, C_LINE, 255);
  /* заголовок: стрілка вгору й назва поточної теки */
  {
    int can = strcmp(root, "/") != 0, hov;
    float mbx = w - 2 * fu;
    int clk = interact(ID, 0, 0, w - hair(), hh, &hov);
    if (hov && can) fill_rrect(6 * fu, 5 * fu, w - 12 * fu, hh - 10 * fu, 5 * fu, C_HOVER, 255);
    draw_tri(14 * fu, hh / 2 - 5 * fu, 10 * fu, 2, can ? (hov ? C_TXT : C_DIM) : C_FAINT);
    char nm[300];
    const char *base = strcmp(root, "/") ? basename_of(root) : "/";
    fit_right(&c_uib, base, mbx - 40 * fu, nm, sizeof nm);
    text(&c_uib, nm, 34 * fu, mid(&c_uib, 0, hh), C_TXT, 0);
    fill_rect(0, hh - hair(), w - hair(), hair(), C_LINE, 255);
    if (clk && can) { tree_up(); focus = F_FILES; }
  }
  float rh = 26 * fu;
  double maxs = nnodes * rh - h + 8 * fu;
  if (tree_scroll > maxs) tree_scroll = maxs;
  if (tree_scroll < 0) tree_scroll = 0;
  clip(0, hh, w - hair(), th - hh);
  int clicked = -1, entered = -1;
  for (int i = 0; i < nnodes; i++) {
    float y = (float)(top + i * rh - tree_scroll);
    if (y + rh < hh || y > th) continue;
    int in = layer == top_layer && inside(0, y, w - 1, rh) && M.y >= hh && M.y < th;
    if (in && M.pressed) {
      /* тека: клік по стрілці розгортає, клік по назві заходить усередину */
      float arrow_end = 14 * fu + nodes[i].depth * 14 * fu + 11 * fu;
      focus = F_FILES; tree_sel = i;
      if (nodes[i].dir && M.x >= arrow_end) entered = i; else clicked = i;
    }
    int is_act = D && !nodes[i].dir && !strcmp(nodes[i].path, D->path);
    int is_open = !nodes[i].dir && find_doc(nodes[i].path) >= 0;
    if (i == tree_sel && focus == F_FILES) fill_rrect(6 * fu, y, w - 12 * fu, rh, 5 * fu, C_HOVER, 255);
    else if (is_act) fill_rrect(6 * fu, y, w - 12 * fu, rh, 5 * fu, C_PANEL2, 255);
    else if (in) fill_rrect(6 * fu, y, w - 12 * fu, rh, 5 * fu, (RGB){35, 34, 31}, 255);
    float ix = 14 * fu + nodes[i].depth * 14 * fu;
    if (nodes[i].dir) draw_tri(ix, y + rh / 2 - 4 * fu, 8 * fu, find_expanded(nodes[i].path) >= 0 ? 1 : 0, C_FAINT);
    draw_icon(ix + 12 * fu, y + rh / 2 - 6.5f * fu, 13 * fu, nodes[i].dir, nodes[i].dir ? (RGB){118, 112, 102} : (is_act ? C_ACC : C_FAINT));
    char nm[300];
    float tx = ix + 32 * fu;
    fit_right(&c_ui, nodes[i].name, w - tx - 10 * fu, nm, sizeof nm);
    text(&c_ui, nm, tx, mid(&c_ui, y, rh), is_act ? C_TXT : is_open ? (RGB){196, 191, 182} : nodes[i].name[0] == '.' ? C_FAINT : C_DIM, 0);
  }
  SDL_RenderSetClipRect(R, NULL);
  if (entered >= 0) tree_enter(entered);
  else if (clicked >= 0) tree_activate(clicked);
}

/* ================= малювання: термінал ================= */
static void draw_term(void) {
  float fu = (float)u, x = g_termx, y = g_termy, w = g_termw, h = g_termh;
  fill_rect(x, y, w, h, C_TERM_BG, 255);
  fill_rect(x, y, w, hair(), C_LINE, 255);
  float padx = 14 * fu, pady = 10 * fu, gx = x + padx, gy = y + pady;
  gcache_size(&c_term, 13.5 * u);
  Font *mf = c_term.f;
  double px = c_term.px64 / 64.0;
  hb_codepoint_t gm = 0;
  hb_font_get_nominal_glyph(mf->hb, 'M', &gm);
  t_cw = (float)(hb_font_get_glyph_h_advance(mf->hb, gm) * px / mf->upem);
  t_ch = (float)ceil(px * 1.35);
  term_resize((int)((h - 2 * pady) / t_ch), (int)((w - 2 * padx) / t_cw));

  double asc = mf->face->ascender * px / mf->upem, desc = -mf->face->descender * px / mf->upem;
  float boff = (float)((t_ch - (asc + desc)) / 2 + asc);
  clip(x, y + hair(), w, h - hair());
  static uint32_t cps[2048];
  for (int r = 0; r < trows; r++) {
    Cell *row;
    int rn, idx = nhist - hview + r;
    if (idx < 0) continue;
    if (idx < nhist) { row = hist[idx].c; rn = hist[idx].n; }
    else { row = &scr[(idx - nhist) * tcols]; rn = tcols; }
    if (rn > tcols) rn = tcols;
    if (rn > 2048) rn = 2048;
    float ry = gy + r * t_ch;
    for (int c = 0; c < rn; c++) {
      Cell *ce = &row[c];
      int inv = ce->fl & CF_INV;
      if (!(ce->fl & CF_DBG) || inv) fill_rect(gx + c * t_cw, ry, t_cw + 0.5f, t_ch, inv ? ((ce->fl & CF_DFG) ? C_TERM_FG : ce->fg) : ce->bg, 255);
    }
    int last = rn;
    for (int c = 0; c < rn; c++) cps[c] = row[c].ch ? row[c].ch : ' ';
    while (last > 0 && cps[last - 1] == ' ') last--;
    if (last > 0) {
      hb_glyph_info_t *inf;
      unsigned g = shape(mf, cps, last, &inf, NULL);
      for (unsigned i = 0; i < g; i++) {
        unsigned cl = inf[i].cluster;
        if ((int)cl >= rn) continue;
        Cell *ce = &row[cl];
        RGB fg = (ce->fl & CF_INV) ? ((ce->fl & CF_DBG) ? C_TERM_BG : ce->bg) : ce->fg;
        blit_glyph(get_glyph(&c_term, inf[i].codepoint), gx + cl * t_cw, ry + boff, fg, 255);
      }
      for (int c = 0; c < rn; c++) if (row[c].fl & CF_UL) fill_rect(gx + c * t_cw, ry + boff + 2 * fu, t_cw, hair(), row[c].fg, 255);
    }
  }
  if (hview == 0 && t_curvis && !term_dead) {
    float cx = gx + tcx * t_cw, cy = gy + tcy * t_ch;
    if (focus == F_TERM && !modal) {
      if (((SDL_GetTicks() - blink_t) / 530) % 2 == 0) {
        fill_rect(cx, cy, t_cw, t_ch, C_ACC, 230);
        uint32_t ch = CELL(tcy, tcx).ch;
        if (ch > ' ') {
          hb_glyph_info_t *inf;
          if (shape(mf, &ch, 1, &inf, NULL)) blit_glyph(get_glyph(&c_term, inf[0].codepoint), cx, cy + boff, C_ACC_DARK, 255);
        }
      }
    } else {
      float b = hair();
      fill_rect(cx, cy, t_cw, b, C_FAINT, 255);
      fill_rect(cx, cy + t_ch - b, t_cw, b, C_FAINT, 255);
      fill_rect(cx, cy, b, t_ch, C_FAINT, 255);
      fill_rect(cx + t_cw - b, cy, b, t_ch, C_FAINT, 255);
    }
  }
  SDL_RenderSetClipRect(R, NULL);
}

/* ================= права панель =================
 * Завжди відкрита, фіксованої ширини, як провідник ліворуч. Нічого не спливає:
 * вгорі — файл і пошук, нижче (прокручується) — вставка, PDF, аркуш, сторінки,
 * документ і клавіші, у самому низу — рядок стану з останнім повідомленням. */
static struct {
  double scroll, want;     /* прокрутка; want >= 0 — перейти до розділу */
  int cap_a, cap_k;        /* яку комбінацію зараз перепризначаємо */
  Field name, author, tz, trows, tcols;
  float sec_y[NSEC];       /* де починається кожен розділ (у координатах вмісту) */
  Doc *name_doc; char name_shown[2048];
  float x, y, w, h, top_h; /* геометрія */
} P = {.cap_a = -1, .want = -1};

static void name_focus(void) {
  if (!D) return;
  ffocus = &P.name;
  P.name.all = 1;
  P.name.caret = (int)strlen(P.name.s);
  blink_t = SDL_GetTicks();
}

static void panel_goto(int sec) {
  if (sec == SEC_FIND) { if (D) find_open(0); return; }
  if (sec == SEC_FILE) { name_focus(); return; }
  P.want = P.sec_y[sec];
}

static int panel_field(Field *f) {
  return f == &P.name || f == &P.author || f == &P.tz || f == &P.trows || f == &P.tcols || f == &F.q || f == &F.r;
}

/* Перемикач із кількох варіантів; переносить кнопки на новий рядок, якщо не вміщаються. */
static int seg(int id, float x, float y, float maxw, const char **opts, int n, int cur, float *used_h) {
  float bh = 28 * (float)u, xx = x, yy = y, gap = 5 * (float)u;
  int res = cur;
  for (int i = 0; i < n; i++) {
    float bw = btn_w(opts[i]) - 6 * (float)u;
    if (xx + bw > x + maxw && xx > x) { xx = x; yy += bh + gap; }
    if (button(id + i, xx, yy, bw, bh, opts[i], i == cur ? BT_ON : BT_NORMAL, 1)) res = i;
    xx += bw + gap;
  }
  *used_h = yy - y + bh;
  return res;
}

/* Текст із переносом по словах у заданій ширині; повертає висоту. */
static float wrap_text(GCache *c, const char *t, float x, float y, float w, RGB col, float lh) {
  char line[600] = "", trial[700];
  const char *p = t;
  float yy = y;
  while (*p) {
    const char *sp = strchr(p, ' ');
    size_t wl = sp ? (size_t)(sp - p) : strlen(p);
    snprintf(trial, sizeof trial, "%s%s%.*s", line, line[0] ? " " : "", (int)wl, p);
    if (line[0] && measure(c, trial) > w) {
      text(c, line, x, yy + lh * 0.72f, col, 0);
      yy += lh;
      snprintf(line, sizeof line, "%.*s", (int)wl, p);
    } else snprintf(line, sizeof line, "%s", trial);
    p += wl;
    while (*p == ' ') p++;
  }
  if (line[0]) { text(c, line, x, yy + lh * 0.72f, col, 0); yy += lh; }
  return yy - y;
}

static void sec_title(const char *t, float x, float *yy) {
  text(&c_small, t, x, *yy + 13 * (float)u, C_FAINT, 0);
  *yy += 22 * (float)u;
}

static void label(const char *t, float x, float *yy) {
  text(&c_ui, t, x, *yy + 14 * (float)u, C_DIM, 0);
  *yy += 22 * (float)u;
}

static void hint(const char *t, float x, float w, float *yy) {
  *yy += wrap_text(&c_small, t, x, *yy, w, C_FAINT, 17 * (float)u) + 4 * (float)u;
}

static void doc_rel_path(Doc *d, char *out, size_t n) {
  size_t rl = strlen(root);
  if (!d->path[0]) { out[0] = 0; return; }
  if (!strncmp(d->path, root, rl) && d->path[rl] == '/') snprintf(out, n, "%s", d->path + rl + 1);
  else snprintf(out, n, "%s", d->path);
}

/* --- верхня частина: файл і пошук --- */
static float panel_top(float x, float y, float w) {
  float fu = (float)u, yy = y, bh = 30 * fu;
  sec_title("FILE", x, &yy);
  if (D) {
    char rel[2048];
    doc_rel_path(D, rel, sizeof rel);
    if (ffocus != &P.name && (P.name_doc != D || strcmp(P.name_shown, rel))) {
      field_set(&P.name, rel);
      P.name_doc = D;
      snprintf(P.name_shown, sizeof P.name_shown, "%s", rel);
    }
    draw_field(ID, &P.name, x, yy, w, 32 * fu, "file name, Enter saves");
  } else {
    box(x, yy, w, 32 * fu, 6 * fu, C_FIELD, C_LINE);
    text(&c_ui, "no file open", x + 10 * fu, mid(&c_ui, yy, 32 * fu), C_FAINT, 0);
  }
  yy += 36 * fu;
  char encl[160] = "";
  if (D && !D->ro && !is_media_view(D->view)) snprintf(encl, sizeof encl, "%s · %s", ENC_NAME[D->enc], D->crlf ? "CRLF" : "LF");
  else if (D) snprintf(encl, sizeof encl, "view only · Ctrl+B: another view");
  text(&c_small, encl, x + 2 * fu, yy + 12 * fu, C_FAINT, 0);
  yy += 20 * fu;
  float tw3 = (w - 10 * fu) / 3;
  if (button(ID, x, yy, tw3, bh, "Save", BT_NORMAL, D != NULL)) { act_save(); if (ffocus == &P.name) save_as_name(P.name.s); }
  if (button(ID, x + tw3 + 5 * fu, yy, tw3, bh, "PDF", BT_NORMAL, D != NULL)) act_pdf();
  if (button(ID, x + 2 * (tw3 + 5 * fu), yy, tw3, bh, "Slideshow", BT_NORMAL, 1)) {
    open_slides(tree_sel >= 0 && tree_sel < nnodes && nodes[tree_sel].dir ? nodes[tree_sel].path : root);
    focus = F_EDIT;
  }
  yy += bh + 16 * fu;

  sec_title("FIND", x, &yy);
  float aw = 34 * fu;
  draw_field(ID, &F.q, x, yy, w - aw - 6 * fu, 32 * fu, "find");
  if (button(ID, x + w - aw, yy + 1 * fu, aw, 30 * fu, "Aa", F.icase ? BT_NORMAL : BT_ON, 1)) F.icase = !F.icase;
  if (ffocus == &F.q || ffocus == &F.r) F.open = 1;
  yy += 38 * fu;
  draw_field(ID, &F.r, x, yy, w, 32 * fu, "replace with");
  yy += 38 * fu;
  int nm = 0;
  char cnt[64] = "";
  if (D && F.q.s[0]) {
    find_update();
    nm = F.nm;
    int k = lower_bound(F.m, F.nm, sel_a());
    int on = k < F.nm && F.m[k] == sel_a() && sel_b() - sel_a() == F.qn;
    if (on) F.idx = k;
    snprintf(cnt, sizeof cnt, "%d / %d", on ? k + 1 : 0, F.nm);
  }
  float sb = 30 * fu;
  if (button(ID, x, yy, sb, bh, "", BT_NORMAL, nm > 0)) find_step(-1);
  draw_tri(x + sb / 2 - 5 * fu, yy + bh / 2 - 5 * fu, 10 * fu, 2, nm ? C_TXT : C_FAINT);
  if (button(ID, x + sb + 4 * fu, yy, sb, bh, "", BT_NORMAL, nm > 0)) find_step(1);
  draw_tri(x + sb + 4 * fu + sb / 2 - 5 * fu, yy + bh / 2 - 5 * fu, 10 * fu, 3, nm ? C_TXT : C_FAINT);
  text(&c_small, cnt, x + 2 * sb + 12 * fu, mid(&c_small, yy, bh), F.q.s[0] && !nm ? (RGB){230, 120, 100} : C_DIM, 0);
  float b2 = btn_w("All") - 4 * fu, b1 = btn_w("Replace") - 4 * fu;
  if (button(ID, x + w - b2, yy, b2, bh, "All", BT_NORMAL, nm > 0)) replace_all();
  if (button(ID, x + w - b2 - 5 * fu - b1, yy, b1, bh, "Replace", BT_NORMAL, nm > 0)) replace_one();
  yy += bh + 14 * fu;
  return yy - y;
}

/* --- прокручувана частина --- */
static float panel_body(float x, float y0, float w) {
  float fu = (float)u, yy = y0, uh, bh = 30 * fu;
  int v;
#define SEC(id, t) do { P.sec_y[id] = (float)(yy - y0); sec_title(t, x, &yy); } while (0)
  /* ВСТАВКА */
  SEC(SEC_INSERT, "INSERT AND FORMAT");
  static const char *fl[] = {"Title", "Subtitle", "Section", "Author", "Center", "Right", "Left",
                             "List 1. 2.", "List “-”", "* * *", "Line", "Align table"};
  float cw = (w - 6 * fu) / 2;
  for (int i = 0; i < 12; i++) {
    float bx = x + (i % 2) * (cw + 6 * fu), by = yy + (i / 2) * (bh + 5 * fu);
    if (button(ID + i, bx, by, cw, bh, fl[i], BT_NORMAL, D && !D->ro)) {
      focus = F_EDIT; ffocus = NULL;
      switch (i) {
        case 0: format_lines(FMT_TITLE); break;
        case 1: format_lines(FMT_SUBTITLE); break;
        case 2: format_lines(FMT_SECTION); break;
        case 3: insert_author(); break;
        case 4: format_lines(FMT_CENTER); break;
        case 5: format_lines(FMT_RIGHT); break;
        case 6: format_lines(FMT_LEFT); break;
        case 7: format_lines(FMT_NUMBERS); break;
        case 8: format_lines(FMT_BULLETS); break;
        case 9: insert_own_line("* * *", 1); break;
        case 10: { char b[512]; int n = COLS < 500 ? COLS : 500; memset(b, '-', (size_t)n); b[n] = 0; insert_own_line(b, 0); break; }
        case 11: tbl_from_text(); break;
      }
    }
  }
  yy += 6 * (bh + 5 * fu) + 4 * fu;
  /* таблиця заданого розміру */
  float fw = 46 * fu;
  draw_field(ID, &P.trows, x, yy, fw, bh, "3");
  text(&c_ui, "×", x + fw + 9 * fu, mid(&c_ui, yy, bh), C_DIM, 1);
  draw_field(ID, &P.tcols, x + fw + 18 * fu, yy, fw, bh, "3");
  if (button(ID, x + 2 * fw + 24 * fu, yy, w - 2 * fw - 24 * fu, bh, "Insert table", BT_NORMAL, D && !D->ro)) {
    int tr = atoi(P.trows.s), tc = atoi(P.tcols.s);
    focus = F_EDIT; ffocus = NULL;
    tbl_insert_new(tr > 0 ? (tr > 200 ? 200 : tr) : 3, tc > 0 ? (tc > 30 ? 30 : tc) : 3);
  }
  yy += bh + 6 * fu;
  /* дії з таблицею під курсором; кнопки завжди на місці, активні лише в таблиці */
  {
    int ta, tb, ti, in_t = D && !D->ro && tbl_block(D->cur, &ta, &tb, &ti);
    static const char *tl[] = {"+ row", "+ column", "− row", "− column", "↑ row", "← column", "wider", "narrower"};
    static const int ta_[] = {A_ROW_BELOW, A_COL_RIGHT, A_ROW_DEL, A_COL_DEL, A_ROW_ABOVE, A_COL_LEFT, A_COL_WIDER, A_COL_NARROWER};
    float qw = (w - 3 * 5 * fu) / 4, qh = 26 * fu;
    for (int i = 0; i < 8; i++) {
      float bx = x + (i % 4) * (qw + 5 * fu), by2 = yy + (i / 4) * (qh + 5 * fu);
      if (button(ID + 400 + i, bx, by2, qw, qh, "", BT_NORMAL, in_t)) { focus = F_EDIT; ffocus = NULL; tbl_op(ta_[i]); }
      text(&c_small, tl[i], bx + qw / 2, mid(&c_small, by2, qh), in_t ? C_TXT : C_FAINT, 1);
    }
    yy += 2 * (qh + 5 * fu) + 2 * fu;
    hint("The table adjusts as you type. Tab: next cell, Enter: below; in the last one, a new row.", x, w, &yy);
  }
  yy += 8 * fu;
  label("Date and time (click inserts)", x, &yy);
  for (int f = 0; f < NDATEFMT; f++) {
    char b[128];
    date_format(f, b, sizeof b);
    int hov = 0;
    float rh = 24 * fu;
    if (interact(ID + 100 + f, x, yy, w, rh, &hov) && D && !D->ro) { focus = F_EDIT; ffocus = NULL; insert_date(f); }
    if (hov) fill_rrect(x, yy, w, rh, 5 * fu, C_HOVER, 255);
    char fit[160];
    fit_right(&c_small, b, w - 16 * fu, fit, sizeof fit);
    text(&c_small, fit, x + 8 * fu, mid(&c_small, yy, rh), f == S.datefmt ? C_ACC : C_TXT, 0);
    yy += rh;
  }
  char kb[64], hb[160];
  bind_str(ACTS[A_DATE].b[0], kb, sizeof kb);
  snprintf(hb, sizeof hb, "Yellow: the format for %s; change it under “Document”.", kb);
  yy += 4 * fu;
  hint(hb, x, w, &yy);
  yy += 14 * fu;

  /* PDF */
  SEC(SEC_PDF, "PDF");
  static const char *o_pdf[] = {"As on screen", "Black on white"};
  v = seg(ID, x, yy, w, o_pdf, 2, S.pdfmode, &uh);
  if (v != S.pdfmode) { S.pdfmode = v; save_settings(); }
  yy += uh + 6 * fu;
  if (D) {
    char pp[2400], hh[2600];
    pdf_path_for(D, pp, sizeof pp);
    snprintf(hh, sizeof hh, "%s File: %s", S.pdfmode ? "White paper and pure black type: for a printer." : "Paper and ink as on screen.", basename_of(pp));
    hint(hh, x, w, &yy);
  }
  yy += 14 * fu;

  /* АРКУШ */
  SEC(SEC_SHEET, "PAGE");
  static const char *o_style[] = {"E", "B", "D"}, *o_size[] = {"10", "11", "12", "14"},
                    *o_sp[] = {"1,0", "1,15", "1,5", "2,0"}, *o_mg[] = {"5", "10", "15", "20"},
                    *o_ink[] = {"black", "blue", "red", "sepia"}, *o_pap[] = {"white", "cream", "old"},
                    *o_zoom[] = {"width", "75", "100", "125", "150", "200"}, *o_ui[] = {"auto", "100", "125", "150", "200"};
  static const int zv[] = {0, 75, 100, 125, 150, 200};
  label("TT2020 ribbon", x, &yy);
  v = seg(ID, x, yy, w, o_style, 3, S.style, &uh);
  if (v != S.style) { load_doc_font(v); save_settings(); }
  yy += uh + 10 * fu;
  label("Font size, pt", x, &yy);
  v = seg(ID, x, yy, w, o_size, 4, S.size, &uh);
  if (v != S.size) { S.size = v; recompute_metrics(); save_settings(); }
  yy += uh + 10 * fu;
  label("Line spacing", x, &yy);
  v = seg(ID, x, yy, w, o_sp, 4, S.spacing, &uh);
  if (v != S.spacing) { S.spacing = v; recompute_metrics(); save_settings(); }
  yy += uh + 10 * fu;
  label("Margins, mm", x, &yy);
  v = seg(ID, x, yy, w, o_mg, 4, S.margin, &uh);
  if (v != S.margin) { S.margin = v; recompute_metrics(); save_settings(); }
  yy += uh + 4 * fu;
  char info[160];
  snprintf(info, sizeof info, "A page holds %d characters × %d lines of text.", COLS, TPP);
  hint(info, x, w, &yy);
  yy += 6 * fu;
  label("Ink", x, &yy);
  v = seg(ID, x, yy, w, o_ink, 4, S.ink, &uh);
  if (v != S.ink) { S.ink = v; save_settings(); }
  yy += uh + 10 * fu;
  label("Paper", x, &yy);
  v = seg(ID, x, yy, w, o_pap, 3, S.paper, &uh);
  if (v != S.paper) { S.paper = v; save_settings(); if (v == 2) paper_start(); }
  yy += uh + 10 * fu;
  int zc = -1;
  for (int i = 0; i < 6; i++) if (zv[i] == S.zoom) zc = i;
  label("Zoom, %", x, &yy);
  v = seg(ID, x, yy, w, o_zoom, 6, zc, &uh);
  if (v != zc && v >= 0) { S.zoom = zv[v]; save_settings(); }
  yy += uh + 10 * fu;
  label("Calibrate with a ruler", x, &yy);
  if (button(ID, x, yy, 34 * fu, bh, "−", BT_NORMAL, 1)) calib_step(-1);
  char cb[32];
  snprintf(cb, sizeof cb, "%d%%", S.calib);
  text(&c_ui, cb, x + 34 * fu + 34 * fu, mid(&c_ui, yy, bh), C_TXT, 1);
  if (button(ID, x + 102 * fu, yy, 34 * fu, bh, "+", BT_NORMAL, 1)) calib_step(1);
  yy += bh + 4 * fu;
  hint("At 100% the page should be exactly 21 cm wide on screen.", x, w, &yy);
  yy += 6 * fu;
  label("Interface, %", x, &yy);
  v = seg(ID, x, yy, w, o_ui, 5, S.ui, &uh);
  if (v != S.ui) { S.ui = v; save_settings(); }
  yy += uh + 18 * fu;

  /* СТОРІНКИ */
  SEC(SEC_PAGES, "PAGES");
  static const char *o_m[] = {"off", "screen", "screen and PDF"};
  label("Page number", x, &yy);
  v = seg(ID, x, yy, w, o_m, 3, S.pagenum, &uh);
  if (v != S.pagenum) { S.pagenum = v; recompute_metrics(); save_settings(); }
  yy += uh + 4 * fu;
  hint("Last line of the page, on the right; an even number of digits: 01, 02 … 0100.", x, w, &yy);
  yy += 6 * fu;
  label("Page header", x, &yy);
  v = seg(ID, x, yy, w, o_m, 3, S.header, &uh);
  if (v != S.header) { S.header = v; recompute_metrics(); save_settings(); }
  yy += uh + 4 * fu;
  hint("First line of the page: “page 01 lines 1-54”, not editable. In code, as a comment of that language.", x, w, &yy);
  yy += 6 * fu;
  if (button(ID, x, yy, w, bh, "Comment rules for code", BT_NORMAL, 1)) {
    char cp[1200];
    crules_path(cp, sizeof cp);
    open_file(cp);
    focus = F_EDIT;
  }
  yy += bh + 18 * fu;

  /* ДОКУМЕНТ */
  SEC(SEC_DOC, "DOCUMENT");
  label("Hidden files in the file tree", x, &yy);
  {
    static const char *o_h[] = {"hide", "show"};
    v = seg(ID, x, yy, w, o_h, 2, S.hidden, &uh);
    if (v != S.hidden) { S.hidden = v; save_settings(); tree_rebuild(); }
    yy += uh + 12 * fu;
  }
  label("Author", x, &yy);
  draw_field(ID, &P.author, x, yy, w, 32 * fu, "first and last name");
  if (strcmp(S.author, P.author.s)) { snprintf(S.author, sizeof S.author, "%s", P.author.s); save_settings(); }
  yy += 40 * fu;
  label("Time zone", x, &yy);
  v = seg(ID, x, yy, w, TZ_LBL, NTZ, P.tz.s[0] ? -1 : S.tz, &uh);
  if (v >= 0 && (v != S.tz || P.tz.s[0])) { S.tz = v; field_set(&P.tz, ""); snprintf(S.tzcustom, sizeof S.tzcustom, "%s", ""); save_settings(); }
  yy += uh + 8 * fu;
  draw_field(ID, &P.tz, x, yy, w, 32 * fu, "other: Europe/Paris, Asia/Kolkata…");
  if (strcmp(S.tzcustom, P.tz.s)) { snprintf(S.tzcustom, sizeof S.tzcustom, "%s", P.tz.s); save_settings(); }
  yy += 40 * fu;
  snprintf(hb, sizeof hb, "Date format for %s", kb);
  label(hb, x, &yy);
  for (int f = 0; f < NDATEFMT; f++) {
    char b[128];
    date_format(f, b, sizeof b);
    int hov = 0;
    float rh = 24 * fu;
    if (interact(ID + 200 + f, x, yy, w, rh, &hov)) { S.datefmt = f; save_settings(); }
    if (f == S.datefmt) fill_rrect(x, yy, w, rh, 5 * fu, C_HOVER, 255);
    else if (hov) fill_rrect(x, yy, w, rh, 5 * fu, C_PANEL2, 255);
    char fit[160];
    fit_right(&c_small, b, w - 16 * fu, fit, sizeof fit);
    text(&c_small, fit, x + 8 * fu, mid(&c_small, yy, rh), f == S.datefmt ? C_ACC : C_DIM, 0);
    yy += rh;
  }
  yy += 18 * fu;

  /* КЛАВІШІ */
  SEC(SEC_KEYS, "KEYS");
  hint("Click a shortcut and press a new one. Backspace clears, Esc cancels. Red: already taken.", x, w, &yy);
  yy += 4 * fu;
  static const char *grp[] = {"Everywhere", "Text", "File tree"};
  float sw = (w - 8 * fu) / 3;
  for (int g = 0; g < 3; g++) {
    text(&c_uib, grp[g], x, yy + 16 * fu, C_TXT, 0);
    yy += 26 * fu;
    for (int a = 0; a < NACT; a++) {
      if (ACTS[a].cx != g) continue;
      text(&c_small, ACTS[a].label, x, yy + 13 * fu, C_DIM, 0);
      yy += 18 * fu;
      for (int k = 0; k < 3; k++) {
        float bx = x + k * (sw + 4 * fu), rh = 26 * fu;
        int capt = P.cap_a == a && P.cap_k == k;
        if (button(ID + 300 + a * 3 + k, bx, yy, sw, rh, "", capt ? BT_ON : BT_NORMAL, 1)) { P.cap_a = a; P.cap_k = k; ffocus = NULL; }
        char bs[64], fit[64];
        if (capt) snprintf(bs, sizeof bs, "press…");
        else bind_str(ACTS[a].b[k], bs, sizeof bs);
        RGB col = capt ? C_ACC : !ACTS[a].b[k].sc ? C_FAINT : bind_conflict(a, k) ? (RGB){230, 110, 95} : C_TXT;
        fit_right(&c_small, bs, sw - 8 * fu, fit, sizeof fit);
        text(&c_small, fit, bx + sw / 2, mid(&c_small, yy, rh), col, 1);
      }
      yy += 26 * fu + 8 * fu;
    }
    yy += 8 * fu;
  }
  if (button(ID, x, yy, w, bh, "All keys back to default", BT_GHOST, 1)) { keys_init(); save_settings(); set_toast("Keys are back to default"); }
  yy += bh + 20 * fu;
#undef SEC
  return yy - y0;
}

static void draw_panel(void) {
  float fu = (float)u, x = P.x, w = P.w, h = P.h, pad = 14 * fu, iw = w - 2 * pad;
  fill_rect(x, 0, w, h, C_PANEL, 255);
  fill_rect(x, 0, hair(), h, C_LINE, 255);
  if (!P.trows.s[0] && ffocus != &P.trows) field_set(&P.trows, "3");
  if (!P.tcols.s[0] && ffocus != &P.tcols) field_set(&P.tcols, "3");
  if (ffocus != &P.author && strcmp(P.author.s, S.author)) field_set(&P.author, S.author);
  if (ffocus != &P.tz && strcmp(P.tz.s, S.tzcustom)) field_set(&P.tz, S.tzcustom);
  /* верх */
  float th = panel_top(x + pad, 12 * fu, iw);
  P.top_h = th + 12 * fu;
  fill_rect(x + hair(), P.top_h, w - hair(), hair(), C_LINE, 255);
  /* низ: рядок стану фіксованої висоти */
  float st_h = 58 * fu, st_y = h - st_h;
  /* середина */
  float by = P.top_h + hair(), bh_ = st_y - by;
  if (P.want >= 0) { P.scroll = P.want; P.want = -1; } /* одразу до розділу, без анімації */
  clip(x + hair(), by, w - hair(), bh_);
  ic_on = 1; ic_x = x + hair(); ic_y = by; ic_w = w - hair(); ic_h = bh_;
  float total = panel_body(x + pad, (float)(by + 10 * fu - P.scroll), iw) + 10 * fu;
  ic_on = 0;
  SDL_RenderSetClipRect(R, NULL);
  double maxs = total - bh_;
  if (maxs < 0) maxs = 0;
  if (P.want > maxs) P.want = maxs;
  if (P.scroll > maxs) P.scroll = maxs;
  if (P.scroll < 0) P.scroll = 0;
  if (maxs > 0) {
    float tb = fmaxf(30 * fu, bh_ * bh_ / total), ty = by + (bh_ - tb) * (float)(P.scroll / maxs);
    fill_rrect(x + w - 6 * fu, ty, 3 * fu, tb, 1.5f * fu, (RGB){255, 255, 255}, 34);
  }
  /* рядок стану */
  fill_rect(x + hair(), st_y, w - hair(), hair(), C_LINE, 255);
  Uint32 age = SDL_GetTicks() - toast_until;
  RGB col = toast[0] && age < 6000 ? C_TXT : C_FAINT;
  if (toast[0]) {
    clip(x + pad, st_y + 4 * fu, iw, st_h - 8 * fu);
    wrap_text(&c_small, toast, x + pad, st_y + 8 * fu, iw, col, 17 * fu);
    SDL_RenderSetClipRect(R, NULL);
  }
}

/* Клавіші, коли фокус у полі панелі або йде перепризначення комбінації. */
static int panel_key(SDL_Keysym ks) {
  if (P.cap_a >= 0) {
    SDL_Keycode k = ks.sym;
    if (k == SDLK_LCTRL || k == SDLK_RCTRL || k == SDLK_LSHIFT || k == SDLK_RSHIFT || k == SDLK_LALT || k == SDLK_RALT ||
        k == SDLK_LGUI || k == SDLK_RGUI || k == SDLK_MODE) return 1;
    if (ks.scancode == SDL_SCANCODE_ESCAPE && !mods_of(ks.mod)) { P.cap_a = -1; return 1; }
    if (ks.scancode == SDL_SCANCODE_BACKSPACE && !mods_of(ks.mod)) ACTS[P.cap_a].b[P.cap_k] = (Bind){0, 0};
    else ACTS[P.cap_a].b[P.cap_k] = (Bind){ks.scancode, mods_of(ks.mod)};
    P.cap_a = -1;
    save_settings();
    return 1;
  }
  if (!ffocus || !panel_field(ffocus)) return 0;
  int shift = ks.mod & KMOD_SHIFT, cmd = ks.mod & CMDMOD;
  if (ks.sym == SDLK_ESCAPE) {
    if (ffocus == &P.name) P.name_doc = NULL; /* повернути справжню назву */
    if (ffocus == &F.q || ffocus == &F.r) F.open = 0;
    ffocus = NULL; focus = F_EDIT;
    return 1;
  }
  if (ks.sym == SDLK_RETURN || ks.sym == SDLK_KP_ENTER) {
    if (ffocus == &P.name) { save_as_name(P.name.s); P.name_doc = NULL; ffocus = NULL; focus = F_EDIT; }
    else if (ffocus == &F.q) { if (D) find_step(shift ? -1 : 1); }
    else if (ffocus == &F.r) { if (D) { if (cmd) replace_all(); else replace_one(); } }
    else if (ffocus == &P.trows || ffocus == &P.tcols) { if (D) { focus = F_EDIT; ffocus = NULL; tbl_insert_new(atoi(P.trows.s) > 0 ? atoi(P.trows.s) : 3, atoi(P.tcols.s) > 0 ? atoi(P.tcols.s) : 3); } }
    else { ffocus = NULL; focus = F_EDIT; }
    return 1;
  }
  if (ks.sym == SDLK_TAB && (ffocus == &F.q || ffocus == &F.r)) { ffocus = ffocus == &F.q ? &F.r : &F.q; return 1; }
  if (global_key(ks)) return 1;
  field_key(ffocus, ks);
  return 1;
}

/* ================= кадр ================= */
static double auto_ui_scale(void) {
  double sc = pr > 1.01 ? pr : 1.0;
  float ddpi = 0;
  if (pr <= 1.01 && SDL_GetDisplayDPI(SDL_GetWindowDisplayIndex(W), &ddpi, NULL, NULL) == 0 && ddpi > 110 && ddpi < 400)
    sc = round(ddpi / 96.0 * 4) / 4;
  if (sc > oh / 700.0) sc = oh / 700.0;
  if (sc < 1) sc = 1;
  return sc;
}

/* Пікселів на пункт для справжнього розміру (100%), з урахуванням калібрування. */
static double real_scale(void) { return (96.0 / 72.0) * u * S.calib / 100.0; }

static void update_dims(void) {
  int ww, wh;
  SDL_GetRendererOutputSize(R, &ow, &oh);
  SDL_GetWindowSize(W, &ww, &wh);
  pr = ww > 0 ? (double)ow / ww : 1.0;
  u = S.ui == 0 ? auto_ui_scale() : UIS[S.ui] * (pr > 1.01 ? pr : 1.0);
  side_w = (float)(SIDE_W * u);
  term_h = (float)(TERM_H * u);
  tabs_h = (float)(TABS_H * u);
  float right_w = (float)(RIGHT_W * u);
  g_docx = side_w; g_docy = tabs_h;
  g_docw = (float)ow - side_w - right_w; g_doch = (float)oh - tabs_h - term_h;
  /* термінал — на всю ширину вікна, бічні панелі закінчуються над ним */
  g_termx = 0; g_termy = (float)oh - term_h; g_termw = (float)ow; g_termh = term_h;
  P.x = (float)ow - right_w; P.y = 0; P.w = right_w; P.h = (float)oh - term_h;
  double fit = (g_docw - 2 * PAGE_PAD * u) / PAGE_W;
  double nz = S.zoom ? S.zoom / 100.0 * real_scale() : fit; /* 100% — справжній розмір аркуша */
  if (nz > fit) nz = fit;
  if (nz < 0.2) nz = 0.2;
  if (zoom_s > 0 && fabs(nz - zoom_s) > 1e-9)
    for (int i = 0; i < ndocs; i++) docs[i]->scroll *= nz / zoom_s; /* зберігаємо місце в тексті */
  zoom_s = nz;
}

static void render(void) {
  update_dims();
  gcache_size(&c_ui, 13.5 * u);
  gcache_size(&c_uib, 14 * u);
  gcache_size(&c_small, 12.5 * u);
  SDL_SetRenderDrawBlendMode(R, SDL_BLENDMODE_BLEND);
  SDL_SetRenderDrawColor(R, C_DESK.r, C_DESK.g, C_DESK.b, 255);
  SDL_RenderClear(R);
  top_layer = 0;
  layer = 0;
  draw_doc();
  draw_tabs();
  draw_tree();
  draw_panel();
  draw_term();
  SDL_RenderPresent(R);
  if (M.released) active_id = 0;
  M.pressed = M.released = 0;

  static char last_title[400];
  char t[300], wt[400];
  if (D) { doc_name(D, t, sizeof t); snprintf(wt, sizeof wt, "%s%s — Typewriter", t, D->dirty ? " •" : ""); }
  else snprintf(wt, sizeof wt, "Typewriter");
  if (strcmp(wt, last_title)) { SDL_SetWindowTitle(W, wt); snprintf(last_title, sizeof last_title, "%s", wt); }
}

/* ================= події ================= */
static void handle_event(SDL_Event *e) {
  Uint32 now = SDL_GetTicks();
  if (e->type == EV_PTY) {
    if (e->user.data1) { term_feed(e->user.data1, e->user.code); free(e->user.data1); }
    else term_ended();
    return;
  }
  switch (e->type) {
    case SDL_QUIT: if (font_missing) running = 0; else quit_step(); break;
    case SDL_WINDOWEVENT:
      if (e->window.event == SDL_WINDOWEVENT_FOCUS_GAINED) { ignore_until = now + 150; tree_rebuild(); }
      else if (e->window.event == SDL_WINDOWEVENT_SIZE_CHANGED || e->window.event == SDL_WINDOWEVENT_RESIZED) {
        if (!(SDL_GetWindowFlags(W) & SDL_WINDOW_MAXIMIZED)) SDL_GetWindowSize(W, &norm_w, &norm_h);
      } else if (e->window.event == SDL_WINDOWEVENT_LEAVE) {
        if (doc_drag) MLOG("left the window, release_pending=%d\n", release_pending != 0);
        ptr_out = 1;
        /* Деякі системи (SDL на Wayland) при виході мишки за вікно надсилають підроблене
           «відпускання» кнопки. Якщо воно щойно прийшло — ігноруємо й продовжуємо виділення. */
        if (doc_drag && release_pending) { release_pending = 0; drag_detached = 1; M.down = 1; }
      } else if (e->window.event == SDL_WINDOWEVENT_ENTER) {
        if (doc_drag) MLOG("back in the window, detached=%d\n", drag_detached);
        ptr_out = 0;
        if (drag_detached) end_drag();
      }
      break;
    case SDL_TEXTINPUT: {
      if (now < ignore_until) break;
      int mods = SDL_GetModState();
      if (P.cap_a >= 0) break; /* іде перепризначення клавіші */
      if (ffocus && panel_field(ffocus)) {
        if (!(mods & CMDMOD)) field_text(ffocus, e->text.text);
      } else if (!modal && focus == F_TERM) {
        if (!(mods & (KMOD_CTRL | KMOD_ALT))) { hview = 0; pty_write(e->text.text, strlen(e->text.text)); }
      } else if (!modal && focus == F_EDIT && D) {
        if (mods & CMDMOD) break;
        int n;
        uint32_t *cp = utf8_dec(e->text.text, strlen(e->text.text), &n);
        if (n && !ro_block() && !tbl_type(cp, n)) { smart_pre(); insert(cp, n, 1); smart_post(); }
        free(cp);
      }
      blink_t = now;
      break;
    }
    case SDL_KEYDOWN: {
      if (now < ignore_until) break;
      blink_t = now;
      SDL_Keysym ks = e->key.keysym;
      if (font_missing) { if (ks.sym == SDLK_ESCAPE || hit(A_QUIT, ks)) running = 0; break; }
      if (hit(A_TERM, ks)) { focus = focus == F_TERM ? F_EDIT : F_TERM; ffocus = NULL; break; }
      if (panel_key(ks)) break;
      if (focus == F_TERM) term_key(ks);
      else if (focus == F_FILES) files_key(ks);
      else doc_key(ks);
      break;
    }
    case SDL_MOUSEMOTION: {
      float nx = (float)(e->motion.x * pr), ny = (float)(e->motion.y * pr);
      int in = nx >= 0 && ny >= 0 && nx < ow && ny < oh;
      if (doc_drag) MLOG("motion %d %d state %u%s\n", e->motion.x, e->motion.y, e->motion.state, in ? "" : " (outside the window)");
      if (drag_detached) {
        /* мишка повернулася всередину — виділення закінчено там, де воно було */
        if (in && !near_edge(nx, ny)) end_drag(); else break;
      }
      last_mx = nx; last_my = ny;
      if (in) last_in_y = ny;
      M.x = nx;
      M.y = ny;
      break;
    }
    case SDL_MOUSEBUTTONDOWN: {
      M.x = (float)(e->button.x * pr);
      M.y = (float)(e->button.y * pr);
      M.mods = SDL_GetModState();
      if (e->button.button == SDL_BUTTON_MIDDLE) { int t = tab_at(M.x, M.y); if (t >= 0 && !modal) ask_close(t); break; }
      if (e->button.button != SDL_BUTTON_LEFT) break;
      if (drag_detached || release_pending) end_drag();
      M.down = 1; M.pressed = 1; M.clicks = e->button.clicks;
      P.cap_a = -1;
      if (in_rect(M.x, M.y, g_termx, g_termy, g_termw, g_termh)) { focus = F_TERM; ffocus = NULL; }
      else if (in_rect(M.x, M.y, P.x, 0, P.w, P.h)) { ffocus = NULL; } /* поле під мишкою саме візьме фокус */
      else if (in_rect(M.x, M.y, 0, 0, side_w, (float)oh - term_h)) { focus = F_FILES; ffocus = NULL; }
      else if (in_doc(M.x, M.y)) { focus = F_EDIT; ffocus = NULL; if (D) doc_press(M.x, M.y, e->button.clicks, M.mods & KMOD_SHIFT); }
      break;
    }
    case SDL_MOUSEBUTTONUP:
      if (e->button.button != SDL_BUTTON_LEFT) break;
      if (doc_drag) MLOG("released %d %d\n", e->button.x, e->button.y);
      if (drag_detached) break;
      if (doc_drag) {
        float bx = (float)(e->button.x * pr), by = (float)(e->button.y * pr);
        if (drv_x11) {
          /* під X11 стан кнопки можна перепитати в системи */
          if (SDL_GetGlobalMouseState(NULL, NULL) & SDL_BUTTON_LMASK) { drag_detached = 1; M.down = 1; MLOG("→ button still down, continuing\n"); }
          else end_drag();
          break;
        }
        /* На Wayland «відпускання» біля краю вікна або після виходу за нього —
           це майже завжди система, яка втратила мишку, а не ви. Продовжуємо. */
        if (ptr_out || near_edge(bx, by) || near_edge(last_mx, last_my)) { drag_detached = 1; M.down = 1; MLOG("→ near the edge, continuing\n"); break; }
        /* звичайне відпускання підтверджуємо за 40 мс: якщо разом прийде «вихід», воно підроблене */
        release_pending = now ? now : 1;
        break;
      }
      M.down = 0; M.released = 1;
      break;
    case SDL_MOUSEWHEEL: {
      float dy = e->wheel.preciseY;
      if (in_rect(M.x, M.y, g_termx, g_termy, g_termw, g_termh)) {
        if (!t_alt) { hview += (int)lroundf(dy * 3); if (hview < 0) hview = 0; if (hview > nhist) hview = nhist; }
      } else if (in_rect(M.x, M.y, g_docx, g_docy, g_docw, g_doch)) {
        int md = SDL_GetModState();
        if ((md & CMDMOD) && (md & KMOD_ALT)) { if (dy) calib_step(dy > 0 ? 1 : -1); }
        else if (md & CMDMOD) { if (dy) zoom_step(dy > 0 ? 1 : -1); }
        else if (D) D->scroll -= dy * 60 * u;
      } else if (in_rect(M.x, M.y, P.x, P.top_h, P.w, P.h - P.top_h)) { P.want = -1; P.scroll -= dy * 48 * u; }
      else if (in_rect(M.x, M.y, 0, 0, side_w, (float)oh - term_h)) tree_scroll -= dy * 42 * u;
      break;
    }
    case SDL_DROPFILE:
      if (!modal) {
        if (is_dir(e->drop.file)) { snprintf(root, sizeof root, "%s", e->drop.file); tree_rebuild(); }
        else open_file(e->drop.file);
      }
      SDL_free(e->drop.file);
      break;
    default: break;
  }
}

/* ================= сеанс =================
 * Під час виходу (і щоп'ять секунд, на випадок збою) зберігаємо все, що видно:
 * вкладки, курсори, виділення, прокрутку, дерево тек, пошук, вікно.
 * Незбережений текст кладемо в ~/.local/share/mashynka/unsaved/. */
static void mkdirs(const char *path) {
  char t[1200];
  snprintf(t, sizeof t, "%s", path);
  for (char *q = t + 1; *q; q++)
    if (*q == '/') { *q = 0; mkdir(t, 0755); *q = '/'; }
  mkdir(t, 0755);
}

static void data_dir(char *out, size_t n) {
  const char *x = getenv("XDG_DATA_HOME");
  if (x && *x) snprintf(out, n, "%s/chlorum/typewriter", x);
  else { char h[1024]; home_dir(h, sizeof h, NULL); snprintf(out, n, "%s/.local/share/chlorum/typewriter", h); }
}

static void session_save(void) {
  char dir[1100], ub[1200], sp[1200], tmp[1300], bp[1400];
  data_dir(dir, sizeof dir);
  snprintf(ub, sizeof ub, "%s/unsaved", dir);
  mkdirs(ub);
  snprintf(sp, sizeof sp, "%s/session", dir);
  snprintf(tmp, sizeof tmp, "%s.tmp", sp);
  FILE *f = fopen(tmp, "w");
  if (!f) return;
  int maxed = (SDL_GetWindowFlags(W) & SDL_WINDOW_MAXIMIZED) != 0;
  fprintf(f, "version 1\nroot %s\nactive %d\nfocus %d\ntree_scroll %.1f\nwindow %d %d %d\n",
          root, act, focus == F_TERM ? F_EDIT : focus, tree_scroll, maxed, norm_w, norm_h);
  fprintf(f, "find %d %d %d\nfindq %s\nfindr %s\n", F.open, F.replace, F.icase, F.q.s, F.r.s);
  for (int i = 0; i < nexp; i++) fprintf(f, "expand %s\n", expanded[i]);
  for (int i = 0; i < ndocs; i++) {
    Doc *d = docs[i];
    fprintf(f, "tab\nuid %u\npath %s\nview %d\ncur %d\nanchor %d\nscroll %.2f\n",
            d->uid, d->path, d->view, d->cur, d->anchor, zoom_s > 0 ? d->scroll / zoom_s : 0);
    if (d->dirty) {
      snprintf(bp, sizeof bp, "%s/%u.txt", ub, d->uid);
      if (!d->buf_saved || d->buf_ver != d->ver) {
        size_t n;
        char *t = text_utf8(d->c, 0, d->n, &n);
        char bt[1500];
        snprintf(bt, sizeof bt, "%s.tmp", bp);
        FILE *bf = fopen(bt, "wb");
        if (bf) {
          int ok = fwrite(t, 1, n, bf) == n;
          fclose(bf);
          if (ok && rename(bt, bp) == 0) { d->buf_saved = 1; d->buf_ver = d->ver; }
        }
        free(t);
      }
      fprintf(f, "buffer 1\n");
    } else d->buf_saved = 0;
    fprintf(f, "end\n");
  }
  fclose(f);
  rename(tmp, sp);
  /* прибираємо чернетки вкладок, яких уже немає або які збережено */
  DIR *dd = opendir(ub);
  if (dd) {
    struct dirent *de;
    while ((de = readdir(dd))) {
      unsigned id;
      char tail[8];
      if (sscanf(de->d_name, "%u.%7s", &id, tail) != 2 || strcmp(tail, "txt")) continue;
      int keep = 0;
      for (int i = 0; i < ndocs; i++) if (docs[i]->uid == id && docs[i]->dirty) keep = 1;
      if (!keep) { snprintf(bp, sizeof bp, "%s/%s", ub, de->d_name); unlink(bp); }
    }
    closedir(dd);
  }
}

typedef struct { unsigned uid; char path[2048]; int view, cur, anchor, buffer; double scroll; } STab;
static struct {
  int ok, active, focus, maxed, ww, wh, fopen, frep, ficase;
  double tree_scroll;
  char root[1024], fq[512], fr[512];
  STab *t;
  int nt;
} SS;

/* Читає файл сеансу ще до створення вікна (щоб знати його розмір). */
static void session_read(void) {
  char dir[1100], sp[1200], line[2200];
  data_dir(dir, sizeof dir);
  snprintf(sp, sizeof sp, "%s/session", dir);
  FILE *f = fopen(sp, "r");
  if (!f) return;
  SS.ficase = 1;
  STab *cur = NULL;
  while (fgets(line, sizeof line, f)) {
    line[strcspn(line, "\n")] = 0;
    char *v = strchr(line, ' ');
    if (v) *v++ = 0; else v = line + strlen(line);
    if (!strcmp(line, "version")) SS.ok = 1;
    else if (!strcmp(line, "root")) snprintf(SS.root, sizeof SS.root, "%s", v);
    else if (!strcmp(line, "active")) SS.active = atoi(v);
    else if (!strcmp(line, "focus")) SS.focus = atoi(v);
    else if (!strcmp(line, "tree_scroll")) SS.tree_scroll = atof(v);
    else if (!strcmp(line, "window")) sscanf(v, "%d %d %d", &SS.maxed, &SS.ww, &SS.wh);
    else if (!strcmp(line, "find")) sscanf(v, "%d %d %d", &SS.fopen, &SS.frep, &SS.ficase);
    else if (!strcmp(line, "findq")) snprintf(SS.fq, sizeof SS.fq, "%s", v);
    else if (!strcmp(line, "findr")) snprintf(SS.fr, sizeof SS.fr, "%s", v);
    else if (!strcmp(line, "expand")) { if (*v && find_expanded(v) < 0) toggle_expanded(v); }
    else if (!strcmp(line, "tab")) {
      SS.t = realloc(SS.t, (size_t)(SS.nt + 1) * sizeof(STab));
      cur = &SS.t[SS.nt++];
      memset(cur, 0, sizeof *cur);
    } else if (cur) {
      if (!strcmp(line, "uid")) cur->uid = (unsigned)strtoul(v, NULL, 10);
      else if (!strcmp(line, "path")) snprintf(cur->path, sizeof cur->path, "%s", v);
      else if (!strcmp(line, "view")) cur->view = atoi(v);
      else if (!strcmp(line, "cur")) cur->cur = atoi(v);
      else if (!strcmp(line, "anchor")) cur->anchor = atoi(v);
      else if (!strcmp(line, "scroll")) cur->scroll = atof(v);
      else if (!strcmp(line, "buffer")) cur->buffer = atoi(v);
      else if (!strcmp(line, "end")) cur = NULL;
    }
  }
  fclose(f);
}


/* Відновлює вкладки. Викликати, коли вже відомий масштаб (zoom_s). Повертає кількість вкладок. */
static int session_apply(void) {
  if (!SS.ok) return 0;
  char dir[1100], bp[1400];
  data_dir(dir, sizeof dir);
  int want_act = -1;
  for (int i = 0; i < SS.nt; i++) {
    STab *t = &SS.t[i];
    snprintf(bp, sizeof bp, "%s/unsaved/%u.txt", dir, t->uid);
    int has_buf = t->buffer && file_exists(bp);
    int ok_path = t->path[0] && (t->view == VIEW_SLIDES ? is_dir(t->path) : file_exists(t->path));
    if (!has_buf && !ok_path) continue;
    new_tab();
    /* uid — до завантаження: фонові завдання (картинки, PDF) шукають вкладку за ним */
    if (t->uid) { D->uid = t->uid; if (t->uid > uid_next) uid_next = t->uid; }
    if (has_buf) {
      int enc = ENC_UTF8, crlf = 0;
      if (ok_path) { load_into(D, t->path); enc = D->enc; crlf = D->crlf; } /* кодування беремо з файлу на диску */
      load_into(D, bp);
      D->enc = enc; D->crlf = crlf;
      snprintf(D->path, sizeof D->path, "%s", t->path);
      D->dirty = 1;
    } else if (t->view != VIEW_TEXT) load_view(D, t->path, t->view);
    else load_into(D, t->path);
    D->cur = t->cur < 0 ? 0 : t->cur > D->n ? D->n : t->cur;
    D->anchor = t->anchor < 0 ? 0 : t->anchor > D->n ? D->n : t->anchor;
    D->scroll = t->scroll * zoom_s;
    if (i == SS.active) want_act = act;
    else if (want_act < 0 && i > SS.active) want_act = act;
  }
  if (!ndocs) return 0;
  activate(want_act >= 0 ? want_act : ndocs - 1);
  tree_scroll = SS.tree_scroll;
  focus = SS.focus == F_FILES ? F_FILES : F_EDIT;
  F.icase = SS.ficase;
  field_set(&F.q, SS.fq);
  field_set(&F.r, SS.fr);
  if (SS.fopen) { F.open = 1; F.replace = SS.frep; }
  need_visible = 0;
  return ndocs;
}

static void abs_path(const char *arg, char *out, size_t n) {
  char *rp = realpath(arg, NULL);
  if (rp) { snprintf(out, n, "%s", rp); free(rp); return; }
  if (arg[0] == '/') { snprintf(out, n, "%s", arg); return; }
  char cwd[1024];
  snprintf(out, n, "%s/%s", getcwd(cwd, sizeof cwd) ? cwd : ".", arg);
}

int main(int argc, char **argv) {
  signal(SIGPIPE, SIG_IGN);
  dbg = getenv("MASHYNKA_DEBUG") != NULL;
  /* На Wayland запускаємось нативно, а не через XWayland: так мишку видно й за межами вікна. */
  if (getenv("WAYLAND_DISPLAY") && !getenv("SDL_VIDEODRIVER")) SDL_SetHint("SDL_VIDEODRIVER", "wayland,x11");
  if (SDL_Init(SDL_INIT_VIDEO) != 0) { fprintf(stderr, "SDL: %s\n", SDL_GetError()); return 1; }
  SDL_SetHint(SDL_HINT_IME_SHOW_UI, "1");
  EV_PTY = SDL_RegisterEvents(1);
  FT_Init_FreeType(&ftl);
  hbbuf = hb_buffer_create();
  keys_init();
  load_settings();
  crules_load();
  if (S.paper == 2) paper_start(); /* текстура старого паперу готується у фоні */
  pen_reset();
  uid_next = (unsigned)time(NULL);
  session_read();

  if (!getcwd(root, sizeof root)) snprintf(root, sizeof root, "%s", S.dir);
  if (SS.ok && is_dir(SS.root)) snprintf(root, sizeof root, "%s", SS.root);
  for (int i = 1; i < argc; i++) {
    char p[2048];
    abs_path(argv[i], p, sizeof p);
    if (is_dir(p)) { snprintf(root, sizeof root, "%s", p); break; }
    if (i == 1) dirname_of(p, root, sizeof root);
  }

  if (SS.ok && SS.ww >= 900 && SS.wh >= 640) { norm_w = SS.ww; norm_h = SS.wh; }
  W = SDL_CreateWindow("Typewriter", SDL_WINDOWPOS_CENTERED, SDL_WINDOWPOS_CENTERED, norm_w, norm_h,
                       SDL_WINDOW_RESIZABLE | SDL_WINDOW_ALLOW_HIGHDPI | (!SS.ok || SS.maxed ? SDL_WINDOW_MAXIMIZED : 0));
  if (!W) { fprintf(stderr, "SDL: %s\n", SDL_GetError()); return 1; }
  R = SDL_CreateRenderer(W, -1, SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
  if (!R) R = SDL_CreateRenderer(W, -1, 0);
  if (!R) { fprintf(stderr, "SDL: %s\n", SDL_GetError()); return 1; }
  SDL_SetWindowMinimumSize(W, 900, 640);
  { const char *vd = SDL_GetCurrentVideoDriver(); drv_x11 = vd && !strcmp(vd, "x11"); }
  if (dbg) fprintf(stderr, "[typewriter] video driver: %s\n", SDL_GetCurrentVideoDriver());

  if (!load_doc_font(S.style) && !load_doc_font(0)) { fprintf(stderr, "TT2020 font not found in the fonts folder.\n"); font_missing = 1; }
  load_ui_fonts();
  if (!fui.face) { fprintf(stderr, "No font found for the interface.\n"); return 1; }
  recompute_metrics();
  update_dims();
  session_apply();
  int first_arg_tab = -1;
  for (int i = 1; i < argc; i++) {
    char p[2048];
    abs_path(argv[i], p, sizeof p);
    if (!is_dir(p)) {
      FILE *f = fopen(p, "rb");
      if (f) { fclose(f); open_file(p); }
      else if (find_doc(p) < 0) { if (!D || D->path[0] || D->n) new_tab(); snprintf(D->path, sizeof D->path, "%s", p); }
      else activate(find_doc(p));
      if (first_arg_tab < 0) first_arg_tab = act;
    }
  }
  if (first_arg_tab >= 0) { activate(first_arg_tab); focus = F_EDIT; }
  tree_rebuild();

  SDL_StartTextInput();
  SDL_EventState(SDL_DROPFILE, SDL_ENABLE);
  ignore_until = SDL_GetTicks() + 300;
  blink_t = SDL_GetTicks();
  render();
  term_start();

  while (running) {
    SDL_Event e;
    if (SDL_WaitEventTimeout(&e, doc_drag || sb_drag || release_pending ? 16 : 260)) {
      do handle_event(&e); while (running && SDL_PollEvent(&e));
    }
    if (release_pending && SDL_GetTicks() - release_pending >= 40) end_drag();
    jobs_collect();
    if (!modal && SDL_GetTicks() - tree_t > 2500) tree_rebuild();
    static Uint32 sess_t;
    if (running && SDL_GetTicks() - sess_t > 5000) { session_save(); sess_t = SDL_GetTicks(); }
    if (running) render();
  }
  if (child_pid > 0) kill(child_pid, SIGHUP);
  save_settings();
  return 0;
}
