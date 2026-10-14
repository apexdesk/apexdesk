' corpus-source: generated
' corpus-flagged: no
' corpus-prints: big
' corpus-prints: two
' corpus-prints: b
Sub Main()
    Debug.Print IIf(10 > 5, "big", "small")
    Debug.Print Choose(2, "one", "two", "three")
    Debug.Print Switch(1 = 2, "a", 2 = 2, "b")
End Sub
