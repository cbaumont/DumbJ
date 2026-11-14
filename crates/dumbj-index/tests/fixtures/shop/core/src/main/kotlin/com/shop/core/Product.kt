package com.shop.core

data class Product(val name: String, val cost: Money) : Priced {
    override fun price(): Money = cost

    fun discounted(percent: Int): Product {
        val reduced = Money.of(cost.amount() * (100 - percent) / 100)
        return Product(name, reduced)
    }

    companion object {
        const val DEFAULT_NAME = "thing"

        fun sample(): Product = Product(DEFAULT_NAME, Money.ZERO)
    }
}

fun Product.label(): String = "$name: ${formatMoney(cost)}"

fun formatMoney(money: Money): String = money.amount().toString()

object Catalog {
    val items = mutableListOf<Product>()

    fun add(product: Product) {
        items.add(product)
    }
}
