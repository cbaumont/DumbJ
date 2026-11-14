package com.shop.core;

public final class Money {
    public static final Money ZERO = new Money(0);
    private final int amount;

    public Money(int amount) {
        this.amount = amount;
    }

    public static Money of(int amount) {
        return new Money(amount);
    }

    public Money add(Money other) {
        int sum = amount + other.amount;
        return new Money(sum);
    }

    public int amount() {
        return amount;
    }

    public static class Currency {
        public String code;
    }
}
