package com.shop.core;

import java.util.List;

/** A record and an enum, to exercise accessors and library supertypes. */
public record Receipt(Money total, Status status) {
    public enum Status { PAID, REFUNDED }

    public interface Archive extends List<Receipt> {
        Receipt latest();
    }

    static int describe(Receipt receipt, Archive archive) {
        int amount = receipt.total().amount();
        String label = receipt.status().name();
        archive.add(receipt);
        return amount + label.length() + archive.latest().total().amount();
    }
}
