package com.shop.app

import com.shop.core.Catalog
import com.shop.core.Money as Cash
import com.shop.core.Product
import com.shop.core.formatMoney
import com.shop.core.label

class Checkout(private val cart: Cart) {
    fun receipt(): String {
        val total: Cash = cart.price()
        val first = Product.sample()
        Catalog.add(first)
        return formatMoney(total) + first.label()
    }

    fun pay(amount: Int): Cash {
        val paid = Cash.of(amount)
        return paid.add(cart.price())
    }
}
