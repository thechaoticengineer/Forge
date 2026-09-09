// Presentation only. The selected line retains its indentation and trailing space.
function original(value) {
    return value === null || value === undefined ? "" : String(value)
}

function preview(value) {
    const lines = original(value).split(/\r\n|\r|\n/)
    for (let i = 0; i < lines.length; ++i) {
        if (lines[i].trim().length) return lines[i]
    }
    return ""
}

function copySource(value) {
    return original(value)
}

// Text layout cost grows faster than linearly with the length of one line, so a
// single very long line can stall the interface for minutes. These helpers bound
// that cost without changing which characters the user can read or copy.

// No character count can bound what fits in a width: zero-width and combining
// characters render at no advance at all. So the laid-out slice is only a
// starting guess, grown by grownLimit() until the renderer reports the line
// truncated, at which point Text.ElideRight owns the visible cut.
function initialLimit(width, fontSize) {
    return Math.ceil(Math.max(0, width) / Math.max(1, fontSize / 4)) + 64
}

function grownLimit(line, limit) {
    return Math.min(line.length, Math.max(64, limit) * 4)
}

function renderable(line, limit) {
    if (line.length <= limit) return line
    // Never end on a lone high surrogate; the pair is elided away regardless.
    const cut = line.charCodeAt(limit - 1) >= 0xd800 && line.charCodeAt(limit - 1) <= 0xdbff
        ? limit - 1 : limit
    return line.slice(0, cut)
}

// Word wrapping searches for break opportunities inside each line. One unbroken
// line of thousands of characters makes that quadratic, so such text wraps at
// any character instead. Ordinary prose and code keep word wrapping.
function wrapsWords(value) {
    const text = original(value)
    let start = 0
    for (let i = 0; i < text.length; ++i) {
        const code = text.charCodeAt(i)
        if (code !== 10 && code !== 13) continue
        if (i - start > 1000) return false
        start = i + 1
    }
    return text.length - start <= 1000
}
