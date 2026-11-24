package me.edfork.workspace.model

/**
 * Compact RFC-8259 JSON parser/serializer in pure Kotlin so both Android runtime and host JVM
 * unit tests execute the identical JSON parsing code without Android SDK stub issues.
 */
object MiniJson {
    fun parseObject(src: String): Map<String, Any?> {
        val p = Parser(src)
        p.skipWs()
        val v = p.readValue()
        @Suppress("UNCHECKED_CAST")
        return (v as? Map<String, Any?>) ?: error("Expected JSON object")
    }

    fun stringifyObject(entries: Map<String, Any?>): String {
        val sb = StringBuilder(64)
        appendValue(sb, entries)
        return sb.toString()
    }

    private fun appendValue(sb: StringBuilder, v: Any?) {
        when (v) {
            null -> sb.append("null")
            is Boolean, is Number -> sb.append(v.toString())
            is String -> appendString(sb, v)
            is Map<*, *> -> {
                sb.append('{')
                var first = true
                for ((k, valItem) in v) {
                    if (k == null) continue
                    if (!first) sb.append(',')
                    first = false
                    appendString(sb, k.toString())
                    sb.append(':')
                    appendValue(sb, valItem)
                }
                sb.append('}')
            }
            is List<*> -> {
                sb.append('[')
                for (i in v.indices) {
                    if (i > 0) sb.append(',')
                    appendValue(sb, v[i])
                }
                sb.append(']')
            }
            else -> appendString(sb, v.toString())
        }
    }

    private fun appendString(sb: StringBuilder, s: String) {
        sb.append('"')
        for (ch in s) {
            when (ch) {
                '\\' -> sb.append("\\\\")
                '"' -> sb.append("\\\"")
                '\n' -> sb.append("\\n")
                '\r' -> sb.append("\\r")
                '\t' -> sb.append("\\t")
                else -> if (ch.code < 0x20) {
                    sb.append(String.format("\\u%04x", ch.code))
                } else {
                    sb.append(ch)
                }
            }
        }
        sb.append('"')
    }

    private class Parser(private val s: String) {
        private var pos = 0

        fun skipWs() {
            while (pos < s.length && s[pos].isWhitespace()) pos++
        }

        fun readValue(): Any? {
            skipWs()
            if (pos >= s.length) error("Unexpected EOF")
            return when (val c = s[pos]) {
                '{' -> readObject()
                '[' -> readArray()
                '"' -> readString()
                't' -> { expectLiteral("true"); true }
                'f' -> { expectLiteral("false"); false }
                'n' -> { expectLiteral("null"); null }
                '-', in '0'..'9' -> readNumber()
                else -> error("Unexpected char '$c' at $pos")
            }
        }

        private fun expectLiteral(lit: String) {
            if (!s.startsWith(lit, pos)) error("Expected $lit at $pos")
            pos += lit.length
        }

        private fun readObject(): Map<String, Any?> {
            pos++ // '{'
            val map = LinkedHashMap<String, Any?>()
            skipWs()
            if (pos < s.length && s[pos] == '}') {
                pos++
                return map
            }
            while (true) {
                skipWs()
                val key = readString()
                skipWs()
                if (pos >= s.length || s[pos] != ':') error("Expected ':' at $pos")
                pos++
                val value = readValue()
                map[key] = value
                skipWs()
                if (pos >= s.length) error("Unterminated object")
                when (s[pos++]) {
                    '}' -> return map
                    ',' -> continue
                    else -> error("Expected ',' or '}' at ${pos - 1}")
                }
            }
        }

        private fun readArray(): List<Any?> {
            pos++ // '['
            val list = ArrayList<Any?>()
            skipWs()
            if (pos < s.length && s[pos] == ']') {
                pos++
                return list
            }
            while (true) {
                list.add(readValue())
                skipWs()
                if (pos >= s.length) error("Unterminated array")
                when (s[pos++]) {
                    ']' -> return list
                    ',' -> continue
                    else -> error("Expected ',' or ']' at ${pos - 1}")
                }
            }
        }

        private fun readString(): String {
            if (pos >= s.length || s[pos] != '"') error("Expected '\"' at $pos")
            pos++
            val sb = StringBuilder()
            while (pos < s.length) {
                val c = s[pos++]
                when (c) {
                    '"' -> return sb.toString()
                    '\\' -> {
                        if (pos >= s.length) error("Unterminated escape")
                        when (val esc = s[pos++]) {
                            '"', '\\', '/' -> sb.append(esc)
                            'b' -> sb.append('\b')
                            'f' -> sb.append('\u000C')
                            'n' -> sb.append('\n')
                            'r' -> sb.append('\r')
                            't' -> sb.append('\t')
                            'u' -> {
                                val hex = s.substring(pos, (pos + 4).coerceAtMost(s.length))
                                if (hex.length != 4) error("Bad \\u escape")
                                sb.append(hex.toInt(16).toChar())
                                pos += 4
                            }
                            else -> error("Invalid escape \\$esc")
                        }
                    }
                    else -> sb.append(c)
                }
            }
            error("Unterminated string")
        }

        private fun readNumber(): Number {
            val start = pos
            var isFloat = false
            if (s[pos] == '-') pos++
            while (pos < s.length && s[pos] in '0'..'9') pos++
            if (pos < s.length && s[pos] == '.') {
                isFloat = true
                pos++
                while (pos < s.length && s[pos] in '0'..'9') pos++
            }
            if (pos < s.length && (s[pos] == 'e' || s[pos] == 'E')) {
                isFloat = true
                pos++
                if (pos < s.length && (s[pos] == '+' || s[pos] == '-')) pos++
                while (pos < s.length && s[pos] in '0'..'9') pos++
            }
            val token = s.substring(start, pos)
            return if (isFloat) token.toDouble() else token.toLong()
        }
    }
}

@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.str(key: String, default: String = ""): String =
    (this[key] as? String) ?: default

fun Map<String, Any?>.optStr(key: String): String? =
    (this[key] as? String)?.takeIf { it.isNotEmpty() }

fun Map<String, Any?>.bool(key: String, default: Boolean = false): Boolean =
    (this[key] as? Boolean) ?: default

fun Map<String, Any?>.numLong(key: String, default: Long = 0L): Long =
    (this[key] as? Number)?.toLong() ?: default

fun Map<String, Any?>.optLong(key: String): Long? =
    (this[key] as? Number)?.toLong()

fun Map<String, Any?>.numInt(key: String, default: Int = 0): Int =
    (this[key] as? Number)?.toInt() ?: default

@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.obj(key: String): Map<String, Any?>? =
    this[key] as? Map<String, Any?>

@Suppress("UNCHECKED_CAST")
fun Map<String, Any?>.objList(key: String): List<Map<String, Any?>> =
    (this[key] as? List<*>)?.mapNotNull { it as? Map<String, Any?> }.orEmpty()
