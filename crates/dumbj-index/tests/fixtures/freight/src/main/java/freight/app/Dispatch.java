package freight.app;

import freight.Money;
import freight.Product;
import freight.Shipping;
import freight.ShippingKt;

class Dispatch {
    Money cost(Product product) {
        String label = ShippingKt.label(product);
        return Shipping.INSTANCE.rate(label, true);
    }
}
