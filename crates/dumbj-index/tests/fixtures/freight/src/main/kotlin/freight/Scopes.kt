package freight

class Ledger2(val total: Money) {
    fun close(): Money = total
}

class Desk {
    fun close(): String = "desk"

    fun work(ledger: Ledger2?, carrier: NamedCarrier) {
        ledger?.let { it.total.amount }
        carrier.also { it.quote(sample()) }
        ledger?.apply { close() }
        with(carrier) { describe() + name }
        listOf(carrier).forEach { it.name }
        products.filter { it.cost.amount > 0 }.map { p -> p.price() }
        catalogue().first().label()
    }

    val products: List<Product> = emptyList()

    fun catalogue(): List<Product> = products
}
