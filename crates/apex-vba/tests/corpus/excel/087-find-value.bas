' corpus-source: generated
' corpus-flagged: no
' corpus-prints: $B$3
Sub Main()
    Dim f As Range
    Range("B2").Value = "apple"
    Range("B3").Value = "pear"
    Set f = Range("A1:C5").Find(What:="pear", LookIn:=xlValues, LookAt:=xlWhole)
    If Not f Is Nothing Then Debug.Print f.Address
End Sub
