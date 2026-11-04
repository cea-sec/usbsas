rule Barbacane_Test
{
    strings:
        $a = "BARBACANE-YARA-TEST"
    condition:
        $a
}
