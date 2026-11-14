package com.example.demo

import kotlin.math.max

/** A greeter. */
@Deprecated("old")
data class Greeter(val name: String, private var count: Int = 0) : Runnable {
    companion object {
        const val MAX = 16
    }

    override fun run() {
        // line comment
        for (i in 0 until MAX) {
            println("Hello, $name ${count + i}\n")
        }
        val items = listOf("a", "b").map { it.uppercase() }
        val ok: Boolean? = if (items.isEmpty()) null else true
        when (count) {
            0 -> return
            else -> count = max(count, 1)
        }
    }
}

fun <T : Comparable<T>> largest(a: T, b: T): T = if (a > b) a else b
