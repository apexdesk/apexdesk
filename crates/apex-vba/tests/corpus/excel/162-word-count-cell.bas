' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 4
Sub Main()
    Range("A1").Value = "the quick brown fox"
    Debug.Print UBound(Split(Trim(Range("A1").Value), " ")) + 1
End Sub
