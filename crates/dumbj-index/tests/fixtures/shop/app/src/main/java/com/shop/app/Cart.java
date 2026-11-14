package com.shop.app;

import com.shop.core.Money;
import com.shop.core.Priced;
import com.shop.core.Product;
import java.util.ArrayList;
import java.util.List;

public class Cart implements Priced {
    private final List<Product> products = new ArrayList<>();
    private Money total = Money.ZERO;

    public void add(Product product) {
        products.add(product);
        Money cost = product.price();
        total = total.add(cost);
    }

    @Override
    public Money price() {
        return total;
    }

    public int count() {
        int count = products.size();
        return count;
    }

    public Product cheapest() {
        Product best = Product.Companion.sample();
        for (Product candidate : products) {
            if (candidate.price().amount() < best.price().amount()) {
                best = candidate;
            }
        }
        return best;
    }

    public boolean empty() {
        return this.count() == 0 && isFree();
    }
}
