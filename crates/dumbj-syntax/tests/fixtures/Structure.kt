@file:JvmName("Utils")
package com.example.demo

import kotlin.math.max
import java.util.*
import com.example.Other as Alias

typealias Names = List<String>

const val LIMIT = 3

fun String.shout(times: Int = 1): String = uppercase()

interface Shape { fun area(): Double; val name: String }

sealed class Base(val id: Int) : Shape, java.io.Serializable {
    companion object { const val MAX = 1; fun create(): Base = TODO() }
    override fun area(): Double { val local = id; return local.toDouble() }
    var count: Int = 0
        get() = field
    class Nested
    object Single
}

enum class Color(val rgb: Int) { RED(1), GREEN(2); fun paint() {} }

data class User(val name: String, private var age: Int) {
    fun greet(other: User): String {
        val (n, a) = other
        other.name.let { it.length }
        for (i in 0 until LIMIT) { println(i) }
        val f = { x: Int, y: Int -> x + y }
        when (val c = age) { 1 -> c else -> 0 }
        try { } catch (e: Exception) { }
        return Alias.make(n).shout(2) + Base.create().area() + max(1, 2)
    }
}

class Multi {
    constructor(a: Int)
    constructor(vararg names: String, block: () -> Unit = {})
}
