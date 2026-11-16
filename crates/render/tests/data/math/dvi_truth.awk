# Reads `dvitype -output-level=4` output and writes what TeX itself typeset: one tab-separated row
# per glyph (`G font code h v`) and per rule (`R h v width height`), h and v in scaled points
# (sp) from the DVI origin, v down to the glyph's baseline or the rule's bottom edge.
# A rule's reference point is its bottom-left corner (DVI rule commands, `putrule` and `setrule`).
function last_value(line, tag,    s, n, parts) {
    if (!match(line, tag "[^,]*")) return ""
    s = substr(line, RSTART + length(tag), RLENGTH - length(tag))
    n = split(s, parts, "=")
    return parts[n] + 0
}
BEGIN { h = 0; v = 0; font = "" }
{
    if (match($0, /current font is [^ ]+/)) font = substr($0, RSTART + 16, RLENGTH - 16)
    if (match($0, /^level [0-9]+:\(h=-?[0-9]+,v=-?[0-9]+/)) {
        s = substr($0, RSTART, RLENGTH); sub(/^level [0-9]+:\(h=/, "", s); split(s, p, ",v="); h = p[1] + 0; v = p[2] + 0
    }
    emitted = 0
    if (match($0, /^[0-9]+: (set|put)char[0-9]+/)) { code = $0; sub(/^[0-9]+: (set|put)char/, "", code); sub(/[^0-9].*$/, "", code); emitted = 1 }
    else if (match($0, /^[0-9]+: (set|put)[1-4] [0-9]+/)) { code = $0; sub(/^[0-9]+: (set|put)[1-4] /, "", code); sub(/[^0-9].*$/, "", code); emitted = 1 }
    if (emitted) printf "G\t%s\t%d\t%d\t%d\n", font, code, h, v
    if (match($0, /^[0-9]+: (set|put)rule height -?[0-9]+, width -?[0-9]+/)) {
        r = $0; sub(/^[0-9]+: (set|put)rule height /, "", r); split(r, q, ", width "); width = q[2]; sub(/[^-0-9].*$/, "", width); height = q[1] + 0
        printf "R\t%d\t%d\t%d\t%d\n", h, v, width + 0, height
    }
    x = last_value($0, " h:="); if (x != "") h = x
    y = last_value($0, " v:="); if (y != "") v = y
}
