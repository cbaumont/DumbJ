package freight.app

fun balance(ledger: Ledger): Int = if (ledger.isOpen) ledger.total else 0
