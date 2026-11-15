@file:JvmName("RateTables")

package freight

@JvmName("standardRate")
fun standard(): Int = 5

@get:JvmName("isCheap")
val cheap: Boolean = true

class Tariff {
    @get:JvmName("isActive")
    val active: Boolean = true
}
