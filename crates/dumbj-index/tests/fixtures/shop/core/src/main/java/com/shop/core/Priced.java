package com.shop.core;

public interface Priced {
    Money price();

    default boolean isFree() {
        return price().amount() == 0;
    }
}
