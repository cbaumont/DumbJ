package freight.app;

import freight.Money;
import freight.NamedCarrier;
import freight.Shipping;
import freight.ShippingKt;

class Interop {
    int read(NamedCarrier carrier, Money money, Ledger ledger) {
        String name = carrier.getName();
        int amount = money.getAmount();
        Money free = Shipping.INSTANCE.getFree();
        boolean open = ledger.isOpen();
        return amount + name.length() + ShippingKt.sample().getCost().getAmount();
    }
}
