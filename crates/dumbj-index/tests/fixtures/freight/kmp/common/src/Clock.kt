package freight.time

expect fun now(): Long

expect fun now(zone: String): Long

expect object Zone {
    val name: String
}

fun stamp(): String = Zone.name + now() + now("utc")
