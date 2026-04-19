#!/usr/bin/env bash
# padctl-demo.sh — simulated padctl TUI session for VHS recording (107×23 terminal)
# Run with: bash demo/padctl-demo.sh

# ── colours (ANSI true-colour) ────────────────────────────────────────────────
P='\033[38;2;0;230;160m'     # seafoam primary
D='\033[38;2;0;110;75m'      # dim green
A='\033[38;2;255;185;0m'     # amber  (paused)
CY='\033[38;2;0;210;220m'    # cyan   (running / BLE connected)
LM='\033[38;2;120;255;80m'   # lime   (speed gauge fill)
BG='\033[48;2;8;8;12m'       # very dark background
N='\033[0m'                   # reset
B='\033[1m'                   # bold

W=107   # confirmed VHS terminal width
H=23    # confirmed VHS terminal height

# ── primitives ────────────────────────────────────────────────────────────────
go()  { printf "\033[%d;%dH" "$1" "$2"; }
hl()  { local n="${1:-$W}"; printf "%.0s─" $(seq 1 "$n"); }

cls() {
    local r
    printf "\033[2J"
    for r in $(seq 1 "$H"); do
        go "$r" 1; printf "${BG}%${W}s${N}" ""
    done
}

# ── scanning screen ───────────────────────────────────────────────────────────
draw_scan() {
    local label="$1"; shift
    local devices=("$@")
    cls

    go 1 1; printf "${BG}${P}${B} ▓ PADCTL ▓ ${N}${BG}${D}  %s${N}" "$label"
    go 2 1; printf "${BG}${D}$(hl)${N}"

    go  4 1; printf "${BG}${D}  ╭─ devices $(hl $((W-14)))╮${N}"
    local r; for r in $(seq 5 15); do
        go $r 1; printf "${BG}${D}  │%$((W-4))s│${N}" ""
    done
    go 16 1; printf "${BG}${D}  ╰$(hl $((W-4)))╯${N}"

    r=6
    for dev in "${devices[@]}"; do
        go $r 4
        printf "${BG}${CY}  ▶  %-53s${D}%-27s${N}" "${dev%%|*}" "${dev##*|}"
        (( r++ ))
    done

    go $((H)) 1
    printf "${BG}${D} [↑/↓] navigate  [Enter] connect  [r] rescan  [c] config  [q] quit${N}"
}

# ── connecting screen ─────────────────────────────────────────────────────────
draw_connect() {
    cls
    go 1 1; printf "${BG}${P}${B} ▓ PADCTL ▓ ${N}${BG}${D}  connecting...${N}"
    go 2 1; printf "${BG}${D}$(hl)${N}"
    go  5 1; printf "${BG}${CY}  Connecting to ${B}KS-HD-Z1D${N}${BG}${CY} ($1)...${N}"
    go  7 1; printf "${BG}${D}  ↳ Discovering GATT services${N}"
    go  8 1; printf "${BG}${D}  ↳ Subscribing to FTMS Treadmill Data  (0x2ACD)${N}"
    go  9 1; printf "${BG}${D}  ↳ Sending RequestControl${N}"
    go 10 1; printf "${BG}${D}  ↳ Subscribing to Machine Status       (0x2ADA)${N}"
    go 11 1; printf "${BG}${D}  ↳ Probing proprietary step channel    (0xfff1)${N}"
    go $H 1; printf "${BG}${D} [r] cancel  [q] quit${N}"
}

# ── disc art — 36-char inner width, 9 rows ───────────────────────────────────
# Hub indicator cycles to suggest rotation
HUBS=("  ╺━━━━━╸  " "  ╺━━━━━╸  " "  ━━━━━━━  " "  ━━━━━━━  ")

draw_disc() {
    local fr="$1"
    local hub="${HUBS[$((fr % 4))]}"
    local sc="$2"  # ring colour

    go  4 3; printf "${BG}${D}%-36s${N}" "  · ◦ ○  ◎  ○ ◦ ·"
    go  5 3; printf "${BG}${D}%-36s${N}" " ◦                  ◦"
    go  6 3; printf "${BG}${D}%-36s${N}" " ○   ┌────────────┐   ○"
    go  7 3; printf "${BG}${D} ○   │${sc}${hub}${D}│   ○%-10s${N}" ""
    go  8 3; printf "${BG}${D} ○   └────────────┘   ○%-10s${N}" ""
    go  9 3; printf "${BG}${D}%-36s${N}" " ◦                  ◦"
    go 10 3; printf "${BG}${D}%-36s${N}" "  · ◦ ○  ◎  ○ ◦ ·"
}

# ── stats panel (right, inner 65 chars, rows 4-12) ───────────────────────────
draw_stats() {
    local speed="$1" dist="$2" steps="$3" time="$4" kcal="$5"
    local c=43  # right panel inner start col

    go  4 $c; printf "${BG}${D}  %-22s%-22s%-17s${N}" "SPEED" "DISTANCE" "STEPS"
    go  5 $c; printf "${BG}%65s${N}" ""
    go  6 $c; printf "${BG}${P}${B}  %-22s%-22s%-17s${N}" "${speed} km/h" "${dist} km" "$steps"
    go  7 $c; printf "${BG}%65s${N}" ""
    go  8 $c; printf "${BG}${D}  %-22s%-22s${N}" "TIME" "CALORIES"
    go  9 $c; printf "${BG}%65s${N}" ""
    go 10 $c; printf "${BG}${P}${B}  %-22s%-22s${N}" "$time" "${kcal} kcal"
    go 11 $c; printf "${BG}%65s${N}" ""
}

# ── speed gauge ───────────────────────────────────────────────────────────────
draw_gauge() {
    local speed_x10="$1"   # speed × 10 integer (e.g. 30 = 3.0 km/h)
    local bar=84
    local filled=$(( speed_x10 * bar / 60 ))
    local empty=$(( bar - filled ))
    local int_part=$(( speed_x10 / 10 ))
    local dec_part=$(( speed_x10 % 10 ))
    local spd_str="${int_part}.${dec_part}"

    go 14 1; printf "${BG}${D}$(hl)${N}"
    go 15 1; printf "${BG}${P}  %s km/h │${LM}" "$spd_str"
    printf "%.0s█" $(seq 1 "$filled"); printf "${D}"; printf "%.0s░" $(seq 1 "$empty")
    printf "${D}│${P} 6.0 km/h${N}"
    go 16 1; printf "${BG}${D}$(hl)${N}"
}

# ── log panel (rows 17-21) ────────────────────────────────────────────────────
draw_log() {
    local msgs=("$@")
    go 17 1; printf "${BG}${D}─── log $(hl $((W-8)))${N}"
    local r=18
    for msg in "${msgs[@]}"; do
        [ $r -gt 21 ] && break
        go $r 1; printf "${BG}${D} >> ${P}%-$((W-4))s${N}" "$msg"
        (( r++ ))
    done
    while [ $r -le 21 ]; do
        go $r 1; printf "${BG}%${W}s${N}" ""; (( r++ ))
    done
    go 22 1; printf "${BG}${D}$(hl)${N}"
}

# ── full session frame ────────────────────────────────────────────────────────
session_frame() {
    local status="$1" sc="$2" ble_c="$3" speed_x10="$4" speed_str="$5"
    local dist="$6" steps="$7" time="$8" kcal="$9" frame="${10}"
    shift 10; local logs=("$@")

    cls

    # Header
    go 1 1
    printf "${BG}${P}${B} ▓ PADCTL ▓ ${N}${BG} ${sc}[ %s ]${N}${BG}${D}  ·  ${ble_c}BLE ●${N}${BG}${P} KS-HD-Z1D${N}" "$status"
    go 2 1; printf "${BG}${D}$(hl)${N}"

    # Left panel (disc) — 40 chars wide, col 1-40
    go  3 1; printf "${BG}${D}  ╭─ disc $(hl 29)╮${N}"
    local r; for r in $(seq 4 12); do
        go $r 1; printf "${BG}${D}  │%36s│${N}" ""
    done
    go 13 1; printf "${BG}${D}  ╰$(hl 36)╯${N}"

    # Right panel (stats) — 67 chars wide, col 41-107
    go  3 41; printf "${BG}${D}╭─ session $(hl 55)╮${N}"
    for r in $(seq 4 12); do
        go $r 41; printf "${BG}${D}│%65s│${N}" ""
    done
    go 13 41; printf "${BG}${D}╰$(hl 65)╯${N}"

    draw_disc "$frame" "$sc"
    draw_stats "$speed_str" "$dist" "$steps" "$time" "$kcal"
    draw_gauge "$speed_x10"
    draw_log "${logs[@]}"

    # Footer
    go $H 1
    case "$status" in
        RUNNING) printf "${BG}${D} [p] PAUSE  [↑/k] +0.1 km/h  [↓/j] -0.1  [h] min  [l] max  [c] config  [q] quit${N}" ;;
        PAUSED)  printf "${BG}${A} [r] RESUME  [x] end session  [c] config  [q] quit${N}" ;;
        IDLE)    printf "${BG}${D} [s] START  [c] config  [q] quit${N}" ;;
    esac
}

# ── session summary (full screen) ─────────────────────────────────────────────
draw_summary() {
    local time="$1" dist="$2" steps="$3" kcal="$4"
    cls
    go 1 1; printf "${BG}${P}${B} ▓ PADCTL ▓ ${N}${BG}${D}  session complete${N}"
    go 2 1; printf "${BG}${D}$(hl)${N}"

    go  4 1; printf "${BG}${D}  ╭─ session summary $(hl $((W-22)))╮${N}"
    local r; for r in $(seq 5 15); do
        go $r 1; printf "${BG}${D}  │%$((W-4))s│${N}" ""
    done
    go 16 1; printf "${BG}${D}  ╰$(hl $((W-4)))╯${N}"

    go  6 5; printf "${BG}${D}  %-24s  %-24s  %-20s  %-16s${N}" "TIME" "DISTANCE" "STEPS" "CALORIES"
    go  8 5; printf "${BG}${P}${B}  %-24s  %-24s  %-20s  %-16s${N}" "$time" "${dist} km" "$steps" "${kcal} kcal"

    go 12 3; printf "${BG}${D}  ╭$(hl $((W-6)))╮${N}"
    go 13 3; printf "${BG}${CY}  │  Sync this session to Google Fit?   ${B}[y]${N}${BG}${CY} Yes   ${B}[n/x]${N}${BG}${CY} No%$((W-61))s│${N}" ""
    go 14 3; printf "${BG}${D}  ╰$(hl $((W-6)))╯${N}"

    go $H 1; printf "${BG}${D} [y] sync to Google Fit  [n/x] discard  [q] quit${N}"
}

# ═════════════════════════════════════════════════════════════════════════════
# DEMO SEQUENCE
# ═════════════════════════════════════════════════════════════════════════════

printf "\033[?25l"   # hide cursor
trap 'printf "\033[?25h\033[0m"' EXIT

# ── Scanning: empty ───────────────────────────────────────────────────────────
draw_scan "scanning for FTMS treadmills..."
sleep 1.5

# ── Scanning: device appears ──────────────────────────────────────────────────
draw_scan "scanning for FTMS treadmills..." \
    "KS-HD-Z1D                                         |82:20:00:24:35:49    -63 dBm"
sleep 1.2

draw_scan "auto-connecting to KS-HD-Z1D..." \
    "KS-HD-Z1D                                         |82:20:00:24:35:49    -63 dBm"
sleep 0.8

# ── Connecting ────────────────────────────────────────────────────────────────
draw_connect "82:20:00:24:35:49"
sleep 2.0

# ── Session: IDLE ─────────────────────────────────────────────────────────────
LOGS=("BLE connected — belt ready" "FTMS RequestControl OK" "Awaiting start  [s]...")
session_frame "IDLE" "$D" "$CY" 0 "—" "0.00" "0" "0:00:00" "0" 0 "${LOGS[@]}"
sleep 1.2

# ── Session: RUNNING @ 3.0 km/h ──────────────────────────────────────────────
LOGS=("BLE connected — belt ready" "FTMS RequestControl OK" "Belt started" "Speed → 3.0 km/h")

TIMES=("0:00:01" "0:00:02" "0:00:03" "0:00:04" "0:00:05" "0:00:06" "0:00:07" "0:00:08")
DISTS=("0.01"    "0.01"    "0.02"    "0.03"    "0.03"    "0.04"    "0.05"    "0.05")
STEPS=(  "14"    "28"      "42"      "57"      "71"      "85"      "99"     "113")
KCALS=(   "1"     "2"       "2"       "3"       "4"       "4"       "5"       "6")

for i in $(seq 0 7); do
    session_frame "RUNNING" "$CY" "$CY" 30 "3.0" \
        "${DISTS[$i]}" "${STEPS[$i]}" "${TIMES[$i]}" "${KCALS[$i]}" \
        "$i" "${LOGS[@]}"
    sleep 0.5
done

# ── Speed ramp: 3.0 → 3.5 → 4.0 km/h ────────────────────────────────────────
LOGS=("Belt started" "Speed → 3.0 km/h" "Speed → 3.5 km/h")
session_frame "RUNNING" "$CY" "$CY" 35 "3.5" "0.06" "127" "0:00:09" "7" 0 "${LOGS[@]}"
sleep 0.6

LOGS=("Speed → 3.0 km/h" "Speed → 3.5 km/h" "Speed → 4.0 km/h")
session_frame "RUNNING" "$CY" "$CY" 40 "4.0" "0.07" "141" "0:00:10" "1" "${LOGS[@]}"
sleep 0.5

for tick in 1 2 3; do
    session_frame "RUNNING" "$CY" "$CY" 40 "4.0" \
        "0.0$((7+tick))" "$((141+14*tick))" "0:00:$((10+tick))" "$((8+tick))" \
        "$tick" "${LOGS[@]}"
    sleep 0.5
done

# ── PAUSED ────────────────────────────────────────────────────────────────────
LOGS=("Speed → 3.5 km/h" "Speed → 4.0 km/h" "Session paused")
session_frame "PAUSED" "$A" "$CY" 40 "4.0" "0.11" "183" "0:00:14" "0" "${LOGS[@]}"
sleep 2.0

# ── RESUME ────────────────────────────────────────────────────────────────────
LOGS=("Speed → 4.0 km/h" "Session paused" "Session resumed at 4.0 km/h")
for tick in 0 1 2 3; do
    session_frame "RUNNING" "$CY" "$CY" 40 "4.0" \
        "0.1$((1+tick))" "$((183+14*tick))" "0:00:$((14+tick))" "$((12+tick))" \
        "$tick" "${LOGS[@]}"
    sleep 0.5
done

# ── End session ───────────────────────────────────────────────────────────────
LOGS=("Session resumed at 4.0 km/h" "Session ended")
session_frame "PAUSED" "$A" "$CY" 40 "4.0" "0.15" "225" "0:00:18" "1" "${LOGS[@]}"
sleep 0.8

# ── Summary ───────────────────────────────────────────────────────────────────
draw_summary "0:00:18" "0.15" "225" "16"

# Hold at summary until VHS stops recording
read -r -t 20 2>/dev/null || true
