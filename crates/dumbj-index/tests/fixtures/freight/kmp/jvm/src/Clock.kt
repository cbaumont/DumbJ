package freight.time

actual fun now(): Long = System.currentTimeMillis()

actual fun now(zone: String): Long = 0

actual object Zone {
    actual val name: String = "jvm"
}

fun jvmStamp(): String = Zone.name + now()
