package freight

class Money(val amount: Int)

class Product(val name: String, val cost: Money) {
    fun price(): Money = cost
}

fun Product.label(): String = name

interface Carrier {
    fun quote(product: Product): Money
}

fun Carrier(name: String): Carrier = NamedCarrier(name)

class NamedCarrier(val name: String) : Carrier {
    constructor(name: String, surcharge: Money) : this(name)

    override fun quote(product: Product): Money = product.price()
}

open class Parcel {
    fun send(to: String) {}
}

class Crate : Parcel() {
    fun send(to: String, insured: Money) {}
}

object Shipping {
    val free = Money(0)

    fun rate(product: Product): Money = product.price()

    fun rate(money: Money): Money = money

    fun rate(code: String, express: Boolean = false): Money = free

    fun rate(vararg products: Product, discount: Int): Money = free

    fun track(id: String, onUpdate: (String) -> Unit) {}

    fun track(id: String, parcel: Parcel) {}

    fun demo(product: Product, crate: Crate) {
        rate(product)
        rate(product.price())
        rate("EU")
        rate("EU", express = true)
        rate(product, product, discount = 5)
        rate(unknown())
        track("a") { println(it) }
        track("b", crate)
        Carrier("fast").quote(product)
        NamedCarrier("slow", free).name
        NamedCarrier("slower").name
        crate.send("home")
        crate.send("away", free)
    }
}
