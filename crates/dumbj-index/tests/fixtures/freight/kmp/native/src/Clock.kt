package freight.time

actual fun now(): Long = 0

actual fun now(zone: String): Long = 0

actual object Zone {
    actual val name: String = "native"
}
